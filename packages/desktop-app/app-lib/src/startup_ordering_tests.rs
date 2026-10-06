//! The app's startup ordering around the product check (ADR-084 §4.3): the
//! calls the frontend makes at boot wait until a start attempt has found this
//! app's own daemon answering, and never reach a daemon that has not said it
//! is this app's.
//!
//! Each test serves a stand-in daemon on a socket in a temporary directory and
//! records the path of every request it receives, so a test can say exactly
//! which calls reached which daemon. The app is a `MockRuntime` app managing a
//! client built the way the app's own is, and the boot calls are the real
//! Tauri commands. No real daemon, service manager or home directory is used.
//!
//! Nothing here is bounded by the wall clock. A test that needs the hold's
//! limit to pass runs on a paused clock; the others end on what the stand-in
//! daemons answer.

use std::convert::Infallible;
use std::future::Future;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use nodespace_proto::nodespace::{
    GetDaemonVersionRequest, GetDaemonVersionResponse, GetNodeRequest,
};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::{App, Manager, State};
use tokio::net::UnixListener;
use tokio::task::{AbortHandle, JoinHandle};
use tonic::body::BoxBody;
use tonic::codegen::{http, BoxFuture, Service};
use tonic::server::NamedService;
use tonic::transport::Server;

use crate::commands::database::list_databases;
use crate::commands::nodes::{get_node, probe_and_recover};
use crate::daemon_profile::DaemonProfile;
use crate::daemon_setup::{
    answering_daemon, evict_other_product_daemon, start_attempt, Answering, DaemonStatus,
};
use crate::extensions::{assemble, AppExtensions};
use crate::services::startup_hold::{StartupHold, STARTUP_HOLD_LIMIT};
use crate::services::{GrpcClient, StartupHoldState};

const VERSION: &str = "/nodespace.NodeService/GetDaemonVersion";
const GET_NODE: &str = "/nodespace.NodeService/GetNode";
const LIST: &str = "/nodespace.DatabaseService/List";

/// What a daemon of another product reports as its executable.
const OTHER_PRODUCT: &str = "/opt/other/bin/other-daemon";
/// What a daemon of this app's community profile reports.
const THIS_APP: &str = "/Users/someone/.nodespace/bin/nodespaced";

/// How long a test lets the boot calls try to go out before the start attempt
/// runs. It only gives a call that wrongly goes out the time to do so: every
/// assertion holds however long this takes.
const SETTLE: Duration = Duration::from_millis(300);
/// The wait a start attempt is given for a daemon that answers: far longer
/// than any stand-in takes, and never run out.
const ANSWERED: Duration = Duration::from_secs(300);

/// The paths of the requests a stand-in daemon received, in order.
type Seen = Arc<Mutex<Vec<String>>>;

/// A stand-in daemon serving `socket` until it is dropped or aborted.
struct MockDaemon {
    seen: Seen,
    serving: JoinHandle<()>,
}

impl MockDaemon {
    /// Serves `socket`, answering `GetDaemonVersion` with `executable` and every
    /// other call with `NOT_FOUND`.
    fn serve(socket: &Path, executable: &'static str) -> Self {
        Self::serve_answering(socket, Some(executable), 0)
    }

    /// Serves `socket` like [`serve`](Self::serve), but answers
    /// `GetDaemonVersion` with an error, as a daemon too busy to say does.
    fn serve_without_saying(socket: &Path) -> Self {
        Self::serve_answering(socket, None, usize::MAX)
    }

    /// Serves `socket` like [`serve`](Self::serve), once it has answered the
    /// first `unanswered` `GetDaemonVersion` calls with an error.
    fn serve_saying_late(socket: &Path, executable: &'static str, unanswered: usize) -> Self {
        Self::serve_answering(socket, Some(executable), unanswered)
    }

    fn serve_answering(socket: &Path, executable: Option<&'static str>, unanswered: usize) -> Self {
        let listener = UnixListener::bind(socket).expect("bind the stand-in daemon's socket");
        let seen = Seen::default();
        let answers = Answers {
            executable,
            unanswered: Arc::new(AtomicUsize::new(unanswered)),
            seen: Arc::clone(&seen),
        };
        let incoming = futures::stream::unfold(listener, |listener| async move {
            let accepted = listener.accept().await.map(|(stream, _)| stream);
            Some((accepted, listener))
        });
        let serving = tokio::spawn(async move {
            Server::builder()
                .add_service(Named::<NodeServiceName>::new(answers.clone()))
                .add_service(Named::<DatabaseServiceName>::new(answers))
                .serve_with_incoming(incoming)
                .await
                .expect("the stand-in daemon serves");
        });
        Self { seen, serving }
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    /// Stops the daemon, closing its listener, as a boot-out does.
    fn stopper(&self) -> AbortHandle {
        self.serving.abort_handle()
    }

    /// Stops the daemon and waits until its listener is closed.
    async fn stopped(mut self) {
        self.serving.abort();
        let _ = (&mut self.serving).await;
    }
}

impl Drop for MockDaemon {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

#[derive(Clone)]
struct Answers {
    executable: Option<&'static str>,
    /// How many more `GetDaemonVersion` calls get an error for an answer.
    unanswered: Arc<AtomicUsize>,
    seen: Seen,
}

trait ServiceName: Send + 'static {
    const NAME: &'static str;
}

enum NodeServiceName {}

impl ServiceName for NodeServiceName {
    const NAME: &'static str = "nodespace.NodeService";
}

enum DatabaseServiceName {}

impl ServiceName for DatabaseServiceName {
    const NAME: &'static str = "nodespace.DatabaseService";
}

/// Every method of the gRPC service named `N`, answered by [`Answers`].
struct Named<N> {
    answers: Answers,
    name: PhantomData<fn() -> N>,
}

impl<N> Named<N> {
    fn new(answers: Answers) -> Self {
        Self {
            answers,
            name: PhantomData,
        }
    }
}

impl<N> Clone for Named<N> {
    fn clone(&self) -> Self {
        Self::new(self.answers.clone())
    }
}

impl<N: ServiceName> NamedService for Named<N> {
    const NAME: &'static str = N::NAME;
}

impl<N> Service<http::Request<BoxBody>> for Named<N> {
    type Response = http::Response<BoxBody>;
    type Error = Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<BoxBody>) -> Self::Future {
        let path = request.uri().path().to_owned();
        self.answers.seen.lock().unwrap().push(path.clone());
        let executable = self.answers.executable;
        let unanswered = Arc::clone(&self.answers.unanswered);
        Box::pin(async move {
            if path != VERSION {
                return Ok(
                    tonic::Status::not_found("the stand-in daemon holds no data").into_http(),
                );
            }
            let busy = unanswered
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok();
            let Some(executable) = executable.filter(|_| !busy) else {
                return Ok(tonic::Status::unavailable("the stand-in daemon is busy").into_http());
            };
            let version = tower::service_fn(
                move |_: tonic::Request<GetDaemonVersionRequest>| async move {
                    Ok::<_, tonic::Status>(tonic::Response::new(GetDaemonVersionResponse {
                        version: "0.0.0".to_string(),
                        executable_path: executable.to_string(),
                    }))
                },
            );
            let codec =
                tonic::codec::ProstCodec::<GetDaemonVersionResponse, GetDaemonVersionRequest>::default();
            Ok(tonic::server::Grpc::new(codec)
                .unary(version, request)
                .await)
        })
    }
}

/// The app's own client: lazy on `socket`, held the way the app's is.
fn held_client(socket: &Path) -> GrpcClient {
    GrpcClient::lazy_at(socket, StartupHold::holding(STARTUP_HOLD_LIMIT))
}

fn app_managing(client: GrpcClient) -> App<MockRuntime> {
    let app = tauri::test::mock_app();
    app.manage(client);
    app
}

fn community() -> DaemonProfile {
    DaemonProfile::community()
}

/// The first data-plane calls the frontend makes at boot, through the real
/// commands: the registry listing and a routed node read. Reports whether the
/// node read was answered (the stand-in answers `NOT_FOUND`, which the command
/// returns as no node).
async fn boot_calls(client: State<'_, GrpcClient>) -> bool {
    let (node, _listing) = tokio::join!(
        get_node(client.clone(), "boot-node".to_string()),
        list_databases(client)
    );
    matches!(node, Ok(None))
}

/// One routed node read on `client`'s channel, outside any command.
async fn node_read(client: &GrpcClient) -> Result<(), tonic::Status> {
    client
        .client()
        .await
        .get_node(GetNodeRequest {
            node_id: "boot-node".to_string(),
        })
        .await
        .map(|_| ())
}

/// A start attempt on `socket` for the client `app` manages, with `bring_up`
/// standing in for the launcher's steps and no database refusal.
async fn attempt(
    app: &App<MockRuntime>,
    socket: &Path,
    wait: Duration,
    bring_up: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<(DaemonStatus, Option<String>)> {
    let client = app.state::<GrpcClient>();
    start_attempt(
        Some(client.inner()),
        socket,
        &community(),
        wait,
        || false,
        bring_up,
    )
    .await
}

/// Runs `attempt` beside the boot calls and returns its outcome. The boot
/// calls must still be waiting when it is over: a call that ended, answered
/// or failed, fails the test.
async fn while_boot_calls_wait<T>(app: &App<MockRuntime>, attempt: impl Future<Output = T>) -> T {
    let boot = boot_calls(app.state::<GrpcClient>());
    tokio::pin!(boot);
    let outcome = tokio::select! {
        biased;
        answered = &mut boot => {
            panic!("the held boot calls must keep waiting, but ended (answered: {answered})")
        }
        outcome = async {
            tokio::time::sleep(SETTLE).await;
            attempt.await
        } => outcome,
    };
    assert!(
        futures::poll!(&mut boot).is_pending(),
        "the held boot calls must still be waiting once the attempt is over"
    );
    outcome
}

/// A boot-out for a daemon that must be left alone. It cannot fail the test
/// itself (the eviction runs it on a blocking task and ignores a panic), so
/// each caller asserts that nothing was evicted.
fn unexpected_boot_out() -> impl FnOnce() + Send + 'static {
    || panic!("a daemon of this app's binary must not be booted out")
}

/// A boot-out the daemon on the socket survives, as one outside the shared
/// registration does.
fn survived_boot_out() -> impl FnOnce() + Send + 'static {
    || {}
}

/// The paths after the first `checks`, sorted: the calls that followed the
/// product checks. Fails unless those first paths are all the check.
fn after_checks(seen: &[String], checks: usize) -> Vec<String> {
    assert!(seen.len() >= checks, "{seen:?}");
    assert!(
        seen[..checks].iter().all(|path| path == VERSION),
        "the first {checks} calls must be the product check: {seen:?}"
    );
    let mut rest = seen[checks..].to_vec();
    rest.sort();
    rest
}

fn hold_of(app: &App<MockRuntime>) -> StartupHoldState {
    app.state::<GrpcClient>().startup_hold()
}

#[tokio::test]
async fn no_boot_call_reaches_another_products_daemon_and_they_go_to_its_replacement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let other = MockDaemon::serve(&socket, OTHER_PRODUCT);
    let app = app_managing(held_client(&socket));

    let boot_out = other.stopper();
    let mut replacement = None;
    let (answered, outcome) = tokio::join!(boot_calls(app.state::<GrpcClient>()), async {
        tokio::time::sleep(SETTLE).await;
        assert!(
            other.seen().is_empty(),
            "a boot call reached the daemon before the check: {:?}",
            other.seen()
        );
        attempt(&app, &socket, ANSWERED, async {
            let evicted = evict_other_product_daemon(&socket, &socket, &community(), move || {
                boot_out.abort()
            })
            .await;
            assert!(evicted, "the other product's daemon must be booted out");
            assert_eq!(
                hold_of(&app),
                StartupHoldState::Holding,
                "after an eviction the calls wait for the daemon that answers next"
            );
            // What registering this app's daemon leads to: a daemon of this
            // app's binary binds the socket the boot-out cleared.
            replacement = Some(MockDaemon::serve(&socket, THIS_APP));
            Ok(())
        })
        .await
    });

    assert_eq!(outcome.expect("start"), (DaemonStatus::Healthy, None));
    assert_eq!(
        other.seen(),
        [VERSION],
        "the other product's daemon may answer the product check and nothing else"
    );
    assert!(
        answered,
        "the routed node read must be answered by the new daemon"
    );
    let seen = replacement.expect("the replacement was started").seen();
    assert_eq!(after_checks(&seen, 1), [LIST, GET_NODE]);
}

#[tokio::test]
async fn another_products_daemon_that_survives_the_boot_out_keeps_the_hold_on() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let survivor = MockDaemon::serve(&socket, OTHER_PRODUCT);
    let app = app_managing(held_client(&socket));

    let outcome = while_boot_calls_wait(
        &app,
        attempt(&app, &socket, ANSWERED, async {
            let evicted =
                evict_other_product_daemon(&socket, &socket, &community(), survived_boot_out())
                    .await;
            assert!(evicted, "the registration was booted out");
            Ok(())
        }),
    )
    .await;

    assert_eq!(
        outcome.expect("start"),
        (DaemonStatus::NotRunning, Some(OTHER_PRODUCT.to_owned())),
        "the start names the daemon that kept the socket, for the notice"
    );
    assert_eq!(hold_of(&app), StartupHoldState::Holding);
    assert_eq!(
        survivor.seen(),
        [VERSION, VERSION],
        "it answers the check before and after the launcher's steps, and nothing else"
    );
}

#[tokio::test]
async fn another_products_daemon_that_binds_during_the_start_keeps_the_hold_on() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let app = app_managing(held_client(&socket));

    let mut late = None;
    let outcome = while_boot_calls_wait(
        &app,
        attempt(&app, &socket, ANSWERED, async {
            let evicted =
                evict_other_product_daemon(&socket, &socket, &community(), unexpected_boot_out())
                    .await;
            assert!(!evicted, "nothing was on the socket to ask");
            // A daemon the first step could not ask, such as a unit still
            // loading its model, binds the socket while the launcher runs.
            late = Some(MockDaemon::serve(&socket, OTHER_PRODUCT));
            Ok(())
        }),
    )
    .await;

    assert_eq!(
        outcome.expect("start"),
        (DaemonStatus::NotRunning, Some(OTHER_PRODUCT.to_owned()))
    );
    assert_eq!(hold_of(&app), StartupHoldState::Holding);
    assert_eq!(
        late.expect("the late daemon was started").seen(),
        [VERSION],
        "it answers the check and nothing else"
    );
}

#[tokio::test]
async fn boot_calls_wait_for_the_end_of_the_start_and_then_reach_this_apps_daemon() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let own = MockDaemon::serve(&socket, THIS_APP);
    let app = app_managing(held_client(&socket));

    let (answered, outcome) = tokio::join!(boot_calls(app.state::<GrpcClient>()), async {
        tokio::time::sleep(SETTLE).await;
        assert!(
            own.seen().is_empty(),
            "boot calls must wait for the check: {:?}",
            own.seen()
        );
        attempt(&app, &socket, ANSWERED, async {
            let evicted =
                evict_other_product_daemon(&socket, &socket, &community(), unexpected_boot_out())
                    .await;
            assert!(!evicted, "this app's daemon stays");
            assert_eq!(
                hold_of(&app),
                StartupHoldState::Holding,
                "the first step only evicts; it ends no hold"
            );
            assert_eq!(own.seen(), [VERSION], "and no held call went out on it");
            Ok(())
        })
        .await
    });

    assert_eq!(outcome.expect("start"), (DaemonStatus::Healthy, None));
    assert_eq!(hold_of(&app), StartupHoldState::Released);
    assert!(answered, "the routed node read must be answered");
    assert_eq!(
        after_checks(&own.seen(), 2),
        [LIST, GET_NODE],
        "the first step's check, the check that ends the hold, then the boot calls"
    );
}

#[tokio::test]
async fn boot_calls_wait_through_a_cold_start_for_the_apps_own_daemon() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let app = app_managing(held_client(&socket));

    let mut own = None;
    let (answered, outcome) = tokio::join!(boot_calls(app.state::<GrpcClient>()), async {
        tokio::time::sleep(SETTLE).await;
        attempt(&app, &socket, ANSWERED, async {
            let evicted =
                evict_other_product_daemon(&socket, &socket, &community(), unexpected_boot_out())
                    .await;
            assert!(!evicted, "nothing was on the socket to evict");
            assert_eq!(hold_of(&app), StartupHoldState::Holding);
            own = Some(MockDaemon::serve(&socket, THIS_APP));
            Ok(())
        })
        .await
    });

    assert_eq!(outcome.expect("start"), (DaemonStatus::Healthy, None));
    assert!(answered, "the routed node read must be answered");
    let seen = own.expect("the app's daemon was started").seen();
    assert_eq!(after_checks(&seen, 1), [LIST, GET_NODE]);
}

#[tokio::test]
async fn with_nothing_answering_the_hold_ends_and_calls_fail_as_for_a_stopped_daemon() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let app = app_managing(held_client(&socket));

    let (answered, outcome) = tokio::join!(boot_calls(app.state::<GrpcClient>()), async {
        tokio::time::sleep(SETTLE).await;
        attempt(&app, &socket, Duration::ZERO, async { Ok(()) }).await
    });

    assert_eq!(outcome.expect("start"), (DaemonStatus::NotRunning, None));
    assert_eq!(
        hold_of(&app),
        StartupHoldState::Released,
        "with nothing on the socket there is no daemon to keep the calls from"
    );
    assert!(
        !answered,
        "the waiting calls fail: nothing serves the socket"
    );
    let status = node_read(&app.state::<GrpcClient>())
        .await
        .expect_err("nothing serves the socket");
    assert_eq!(status.code(), tonic::Code::Unavailable, "{status}");
}

#[tokio::test]
async fn a_daemon_that_does_not_say_which_it_is_is_asked_again_until_it_does() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let slow = MockDaemon::serve_saying_late(&socket, THIS_APP, 2);
    let app = app_managing(held_client(&socket));

    let (answered, outcome) = tokio::join!(boot_calls(app.state::<GrpcClient>()), async {
        tokio::time::sleep(SETTLE).await;
        attempt(&app, &socket, ANSWERED, async { Ok(()) }).await
    });

    assert_eq!(outcome.expect("start"), (DaemonStatus::Healthy, None));
    assert!(answered);
    assert_eq!(
        after_checks(&slow.seen(), 3),
        [LIST, GET_NODE],
        "two unanswered checks let no call through; the third, answered, does"
    );
}

#[tokio::test]
async fn a_daemon_that_never_says_which_it_is_keeps_the_hold_on_when_the_wait_is_over() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let silent = MockDaemon::serve_without_saying(&socket);
    let app = app_managing(held_client(&socket));

    let outcome = while_boot_calls_wait(
        &app,
        attempt(&app, &socket, Duration::ZERO, async {
            let evicted =
                evict_other_product_daemon(&socket, &socket, &community(), unexpected_boot_out())
                    .await;
            assert!(!evicted, "an unanswered check evicts nothing");
            Ok(())
        }),
    )
    .await;

    assert_eq!(
        outcome.expect("start"),
        (DaemonStatus::Starting, None),
        "the app goes on reporting the daemon as starting"
    );
    assert_eq!(
        hold_of(&app),
        StartupHoldState::Holding,
        "the wait running out is not an answer, so it ends no hold"
    );
    assert_eq!(silent.seen(), [VERSION, VERSION]);
}

#[tokio::test(start_paused = true)]
async fn a_start_that_fails_ends_the_hold_at_once_when_nothing_answers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let app = app_managing(held_client(&socket));
    let started = tokio::time::Instant::now();

    let outcome = attempt(&app, &socket, ANSWERED, async {
        Err(anyhow::anyhow!("the service manager refused"))
    })
    .await;

    assert!(outcome.is_err(), "the failure is passed through");
    assert_eq!(hold_of(&app), StartupHoldState::Released);
    assert_eq!(
        tokio::time::Instant::now(),
        started,
        "a start that failed has no daemon to wait for"
    );
}

#[tokio::test]
async fn a_start_that_fails_beside_another_products_daemon_names_it_and_keeps_the_hold_on() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let other = MockDaemon::serve(&socket, OTHER_PRODUCT);
    let app = app_managing(held_client(&socket));

    let outcome = while_boot_calls_wait(
        &app,
        attempt(&app, &socket, ANSWERED, async {
            Err(anyhow::anyhow!("the service manager refused"))
        }),
    )
    .await;

    assert_eq!(
        outcome.expect("the other daemon is the outcome, not the failure"),
        (DaemonStatus::NotRunning, Some(OTHER_PRODUCT.to_owned()))
    );
    assert_eq!(hold_of(&app), StartupHoldState::Holding);
    assert_eq!(other.seen(), [VERSION]);
}

#[tokio::test]
async fn a_retry_ends_a_hold_that_is_past_its_limit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let own = MockDaemon::serve(&socket, THIS_APP);
    // The first start attempt never ended: the hold is past its limit.
    let client = GrpcClient::lazy_at(&socket, StartupHold::holding(Duration::ZERO));
    let app = app_managing(client.clone());
    assert_eq!(client.startup_hold(), StartupHoldState::Overdue);

    let status = node_read(&client)
        .await
        .expect_err("a call on a hold past its limit fails");
    assert_eq!(status.code(), tonic::Code::Unavailable, "{status}");
    assert!(own.seen().is_empty(), "and reaches no daemon");
    let reported = crate::daemon_status_of(&client, &socket).await;
    assert!(
        reported != "healthy" && reported != "starting",
        "the daemon is reported down, so the banner and its Retry show: {reported}"
    );

    // Retry runs a new start attempt, which finds this app's daemon answering.
    let outcome = attempt(&app, &socket, ANSWERED, async { Ok(()) }).await;

    assert_eq!(outcome.expect("start"), (DaemonStatus::Healthy, None));
    assert_eq!(client.startup_hold(), StartupHoldState::Released);
    assert_eq!(crate::daemon_status_of(&client, &socket).await, "healthy");
    assert!(boot_calls(app.state::<GrpcClient>()).await);
    assert_eq!(after_checks(&own.seen(), 1), [LIST, GET_NODE]);
}

#[tokio::test]
async fn a_retry_ends_a_hold_left_on_once_the_other_daemon_is_gone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let other = MockDaemon::serve(&socket, OTHER_PRODUCT);
    let app = app_managing(held_client(&socket));

    let first = attempt(&app, &socket, ANSWERED, async { Ok(()) }).await;
    assert_eq!(
        first.expect("start"),
        (DaemonStatus::NotRunning, Some(OTHER_PRODUCT.to_owned()))
    );
    assert_eq!(hold_of(&app), StartupHoldState::Holding);

    // The user stops the other daemon and chooses Retry; the launcher then
    // starts this app's own.
    other.stopped().await;
    std::fs::remove_file(&socket).expect("the stopped daemon's socket file");
    let mut own = None;
    let (answered, retried) = tokio::join!(
        boot_calls(app.state::<GrpcClient>()),
        attempt(&app, &socket, ANSWERED, async {
            own = Some(MockDaemon::serve(&socket, THIS_APP));
            Ok(())
        })
    );

    assert_eq!(retried.expect("start"), (DaemonStatus::Healthy, None));
    assert!(answered);
    let seen = own.expect("the app's daemon was started").seen();
    assert_eq!(after_checks(&seen, 1), [LIST, GET_NODE]);
}

#[tokio::test]
async fn with_another_dialed_socket_only_the_dialed_daemon_is_asked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registered = dir.path().join("daemon.sock");
    let dialed = dir.path().join("override.sock");
    let other = MockDaemon::serve(&registered, OTHER_PRODUCT);
    let own = MockDaemon::serve(&dialed, THIS_APP);
    let app = app_managing(held_client(&dialed));

    let outcome = attempt(&app, &dialed, ANSWERED, async {
        let evicted =
            evict_other_product_daemon(&registered, &dialed, &community(), unexpected_boot_out())
                .await;
        assert!(!evicted);
        Ok(())
    })
    .await;

    assert_eq!(outcome.expect("start"), (DaemonStatus::Healthy, None));
    assert!(
        other.seen().is_empty(),
        "the daemon this app registers says nothing about the one it dials, so it is not asked"
    );
    assert_eq!(hold_of(&app), StartupHoldState::Released);
    assert_eq!(own.seen(), [VERSION]);
}

#[tokio::test]
async fn a_client_built_released_calls_the_daemon_at_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let own = MockDaemon::serve(&socket, THIS_APP);
    let app = app_managing(GrpcClient::lazy_at(&socket, StartupHold::released()));

    let answered = boot_calls(app.state::<GrpcClient>()).await;

    assert!(answered);
    assert_eq!(after_checks(&own.seen(), 0), [LIST, GET_NODE]);
}

#[tokio::test]
async fn the_apps_own_client_is_built_held() {
    assert_eq!(
        GrpcClient::connect_lazy_held().startup_hold(),
        StartupHoldState::Holding
    );
}

/// The one line that turns the hold on: the app manages its own client held,
/// on the targets whose setup task runs the start attempt that ends it.
#[test]
fn the_app_manages_its_own_client_held() {
    let lib = include_str!("lib.rs");
    let manage = lib
        .find("app.manage(crate::services::GrpcClient::connect_lazy_held());")
        .expect("the app manages a held client");
    let cfg = lib[..manage]
        .rfind("#[cfg(")
        .expect("the held client is managed under a cfg");
    assert_eq!(
        &lib[cfg..manage].trim_end(),
        &r#"#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]"#,
        "only the targets that run the start attempt hold the client"
    );
}

/// Nothing starts the daemon around the start attempt that ends the hold. The
/// launcher's steps have one caller, inside the attempt, and the database
/// reset starts the daemon through it like the app's startup and Retry do.
/// The real launcher drives this machine's service manager, so the wiring is
/// pinned here and the behaviour in the tests above.
#[test]
fn every_start_of_the_daemon_goes_through_the_start_attempt() {
    let setup = include_str!("daemon_setup.rs");
    let ensure = setup
        .find("pub async fn ensure_daemon_running(")
        .expect("ensure_daemon_running");
    let body = &setup[ensure..ensure + setup[ensure..].find("\n}\n").expect("its end")];
    let attempt = body
        .find("start_attempt(")
        .expect("it runs a start attempt");
    // The start attempt's arguments: up to the parenthesis that closes it.
    let mut depth = 0usize;
    let arguments_end = body[attempt..]
        .char_indices()
        .find_map(|(i, ch)| {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(attempt + i);
                    }
                }
                _ => {}
            }
            None
        })
        .expect("the start attempt's call closes");
    assert!(
        body[attempt..arguments_end].contains("bring_up_daemon(app)"),
        "the launcher's steps are the start attempt's own"
    );
    assert_eq!(
        setup.matches("bring_up_daemon(app)").count(),
        1,
        "the launcher's steps run nowhere else"
    );
    assert_eq!(
        setup.matches("ensure_daemon_running(app)").count(),
        1,
        "start_daemon_and_report is the one caller"
    );
    assert!(
        include_str!("incompatible_database.rs")
            .contains("daemon_setup::start_daemon_and_report(app).await"),
        "the database reset starts the daemon through the start attempt"
    );
    assert!(
        include_str!("coexistence.rs")
            .contains("crate::daemon_setup::start_daemon_and_report(&app).await"),
        "Retry starts the daemon through the start attempt"
    );
}

#[tokio::test]
async fn the_channel_probe_neither_probes_nor_rebuilds_while_calls_are_held() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let daemon = MockDaemon::serve(&socket, THIS_APP);
    let rebuilds = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&rebuilds);
    let app = assemble(
        mock_builder(),
        AppExtensions::none().on_channel_rebuilt(move |_app, _channel| {
            counted.fetch_add(1, Ordering::SeqCst);
            async {}
        }),
    )
    .build(mock_context(noop_assets()))
    .expect("the app builds");
    let client = held_client(&socket);

    // The frontend's poll probes the channel only on a healthy status, and a
    // daemon answers on the socket.
    assert_eq!(crate::daemon_status_of(&client, &socket).await, "starting");

    let recovered = probe_and_recover(app.handle(), &client).await;

    assert!(!recovered, "nothing is rebuilt");
    assert_eq!(
        rebuilds.load(Ordering::SeqCst),
        0,
        "no channel-rebuilt hook runs"
    );
    assert!(
        daemon.seen().is_empty(),
        "no probe was sent: {:?}",
        daemon.seen()
    );
    assert_eq!(client.startup_hold(), StartupHoldState::Holding);

    // Once the hold ends, the probe goes out again and finds the channel live.
    client.release_startup_hold();
    let recovered = probe_and_recover(app.handle(), &client).await;
    assert!(!recovered);
    assert_eq!(daemon.seen(), [GET_NODE]);
    assert_eq!(rebuilds.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn a_held_call_fails_unavailable_only_once_the_limit_has_passed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let daemon = MockDaemon::serve(&socket, THIS_APP);
    let client = GrpcClient::lazy_at(&socket, StartupHold::holding(Duration::from_secs(60)));
    let call = tokio::spawn({
        let client = client.clone();
        async move { node_read(&client).await }
    });

    tokio::time::sleep(Duration::from_secs(59)).await;
    assert!(
        !call.is_finished(),
        "a held call waits: nothing but the limit fails it"
    );
    assert_eq!(client.startup_hold(), StartupHoldState::Holding);

    tokio::time::sleep(Duration::from_secs(1)).await;
    let status = call
        .await
        .expect("the call does not panic")
        .expect_err("a call held past the limit fails");
    assert_eq!(
        status.code(),
        tonic::Code::Unavailable,
        "it fails the way a call does when nothing serves the socket: {status}"
    );
    assert!(
        daemon.seen().is_empty(),
        "the limit fails a held call, it never lets one through: {:?}",
        daemon.seen()
    );
    assert_eq!(client.startup_hold(), StartupHoldState::Overdue);
}

#[tokio::test(start_paused = true)]
async fn a_held_call_goes_out_when_the_hold_is_released() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let client = GrpcClient::lazy_at(&socket, StartupHold::holding(Duration::from_secs(60)));
    let started = tokio::time::Instant::now();
    let call = tokio::spawn({
        let client = client.clone();
        async move { node_read(&client).await }
    });
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(!call.is_finished(), "the call waits while the hold is on");

    client.release_startup_hold();

    // Nothing serves the socket, so the dial the release lets through is
    // refused: the call ends on the socket's answer, not on the hold's limit.
    let status = call
        .await
        .expect("the call does not panic")
        .expect_err("nothing serves the socket");
    assert_eq!(status.code(), tonic::Code::Unavailable, "{status}");
    assert_eq!(
        tokio::time::Instant::now() - started,
        Duration::from_secs(30),
        "the released call went out at the release, not at the limit"
    );
}

#[tokio::test(start_paused = true)]
async fn the_wait_for_a_daemon_ends_at_once_when_the_daemon_refused_its_database() {
    let dir = tempfile::tempdir().expect("tempdir");
    let started = tokio::time::Instant::now();

    let answering = answering_daemon(
        &dir.path().join("absent.sock"),
        &community(),
        Duration::from_secs(30),
        || true,
    )
    .await;

    assert_eq!(answering, Answering::Nobody);
    assert_eq!(
        tokio::time::Instant::now(),
        started,
        "a daemon that stopped on purpose will never answer, so nothing waits for it"
    );
}

#[tokio::test(start_paused = true)]
async fn without_a_refusal_the_wait_for_a_daemon_runs_its_whole_length() {
    let dir = tempfile::tempdir().expect("tempdir");
    let started = tokio::time::Instant::now();

    let answering = answering_daemon(
        &dir.path().join("absent.sock"),
        &community(),
        Duration::from_secs(30),
        || false,
    )
    .await;

    assert_eq!(answering, Answering::Nobody);
    assert!(tokio::time::Instant::now() - started >= Duration::from_secs(30));
}
