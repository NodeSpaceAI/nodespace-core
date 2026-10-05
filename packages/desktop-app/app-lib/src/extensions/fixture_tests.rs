//! Fixture extension driving the extension API through `assemble` on
//! `tauri::test::MockRuntime`.
//!
//! The fixture is a Tauri plugin with a command and managed state, the shape
//! any extension takes. Later additions to the extension API extend this
//! fixture, and the check that keeps the Rust and TypeScript API versions in
//! step watches it.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use tauri::plugin::{Builder as PluginBuilder, Plugin, TauriPlugin};
use tauri::test::{
    get_ipc_response, mock_app, mock_builder, mock_context, noop_assets, MockRuntime,
};
use tauri::utils::acl::ExecutionContext;
use tauri::webview::InvokeRequest;
use tauri::{App, AppHandle, Manager, Runtime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use super::{
    assemble, core_plugin_names, run_channel_rebuilt_hooks, spawn_daemon_ready_tasks,
    AppExtensions, CancellationToken, Channel, LatestVersionSource, UpdateSource,
    CHANNEL_REBUILT_HOOK_TIMEOUT, EXTENSION_API_VERSION,
};
use crate::services::GrpcClient;
use crate::update_check::{
    builtin_update_source, check_for_update_for_app, update_source_for_app, UpdateSourceState,
};

/// Name of the fixture plugin, which is also its command namespace.
const FIXTURE: &str = "fixture";

/// Managed by the fixture plugin's setup.
struct FixtureState {
    reply: &'static str,
}

#[tauri::command]
fn fixture_ping(state: tauri::State<'_, FixtureState>) -> String {
    state.reply.to_string()
}

/// The fixture extension: one command, and state managed during setup.
fn fixture_plugin<R: Runtime>() -> TauriPlugin<R> {
    PluginBuilder::<R>::new(FIXTURE)
        .invoke_handler(tauri::generate_handler![fixture_ping])
        .setup(|app, _api| {
            app.manage(FixtureState { reply: "pong" });
            Ok(())
        })
        .build()
}

/// A plugin that only records that its setup ran.
fn probe_plugin<R: Runtime>(name: &'static str, ran: &Arc<AtomicBool>) -> TauriPlugin<R> {
    let ran = Arc::clone(ran);
    PluginBuilder::<R>::new(name)
        .setup(move |_app, _api| {
            ran.store(true, Ordering::SeqCst);
            Ok(())
        })
        .build()
}

/// Builds an app from `ext` with no capability granted.
fn build_app(ext: AppExtensions<MockRuntime>) -> App<MockRuntime> {
    assemble(mock_builder(), ext)
        .build(mock_context(noop_assets()))
        .expect("the app builds")
}

/// Builds an app from `ext`, granting `command` to every window.
///
/// The mock context has an empty ACL, and a plugin command is refused unless
/// the ACL allows it. Tauri's only way to allow one on a mock context is this
/// doc-hidden call on the context's runtime authority.
fn build_app_granting(ext: AppExtensions<MockRuntime>, command: &str) -> App<MockRuntime> {
    let mut context = mock_context(noop_assets());
    context
        .runtime_authority_mut()
        .__allow_command(command.to_string(), ExecutionContext::Local);
    assemble(mock_builder(), ext)
        .build(context)
        .expect("the app builds")
}

/// Invokes `command` from a `main` webview the way the frontend would.
fn invoke(app: &App<MockRuntime>, command: &str) -> Result<String, serde_json::Value> {
    let webview = tauri::WebviewWindowBuilder::new(app, "main", Default::default())
        .build()
        .expect("the main window builds");
    let url = if cfg!(any(windows, target_os = "android")) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };
    let request = InvokeRequest {
        cmd: command.into(),
        callback: tauri::ipc::CallbackFn(0),
        error: tauri::ipc::CallbackFn(1),
        url: url.parse().expect("a valid URL"),
        body: tauri::ipc::InvokeBody::default(),
        headers: Default::default(),
        invoke_key: tauri::test::INVOKE_KEY.to_string(),
    };
    get_ipc_response(&webview, request)
        .map(|body| body.deserialize::<String>().expect("a string reply"))
}

#[test]
fn extension_api_version_is_two_two() {
    assert_eq!(EXTENSION_API_VERSION, (2, 2));
}

/// The fixture naming a daemon of its own. `run` installs the profile into
/// the process-wide cell, which these tests never write: they check what an
/// extension carries to `run`, and that `assemble` leaves the profile alone.
mod daemon_profile {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use tauri::test::MockRuntime;
    use tauri::Manager;

    use super::{build_app, fixture_plugin, FixtureState};
    use crate::daemon_profile::{active, DaemonProfile};
    use crate::daemon_setup::profile_for_this_build;
    use crate::extensions::AppExtensions;

    /// A daemon with its own binary and service environment.
    fn fixture_daemon_profile() -> DaemonProfile {
        DaemonProfile {
            binary_name: "fixture-daemon",
            service_env: vec![("FIXTURE_DAEMON_MODE".to_string(), "fixture".to_string())],
        }
    }

    #[test]
    fn none_carries_no_daemon_profile() {
        assert_eq!(
            AppExtensions::<MockRuntime>::none().take_daemon_profile(),
            None,
            "with no profile, run installs the build's default"
        );
    }

    #[test]
    fn a_named_daemon_profile_is_carried_to_run_and_the_last_call_wins() {
        let first = DaemonProfile {
            binary_name: "first-daemon",
            ..fixture_daemon_profile()
        };
        let mut extensions = AppExtensions::<MockRuntime>::none()
            .daemon_profile(first)
            .daemon_profile(fixture_daemon_profile());

        assert_eq!(
            extensions.take_daemon_profile(),
            Some(fixture_daemon_profile())
        );
        assert_eq!(
            extensions.take_daemon_profile(),
            None,
            "taking the profile leaves none behind"
        );
    }

    /// Both of core's own variables are refused, each by name, so a profile
    /// can never replace the socket or the GUI binary the launcher sets.
    #[test]
    fn a_service_env_key_core_sets_itself_is_refused() {
        for key in ["NODESPACED_SOCKET", "NODESPACE_UI_BINARY"] {
            let mut profile = fixture_daemon_profile();
            profile
                .service_env
                .push((key.to_string(), "/elsewhere".to_string()));

            let refused = catch_unwind(AssertUnwindSafe(|| {
                AppExtensions::<MockRuntime>::none().daemon_profile(profile)
            }));
            let message = refused
                .err()
                .and_then(|panic| panic.downcast::<String>().ok())
                .unwrap_or_else(|| panic!("a profile setting `{key}` must be refused"));
            assert!(
                message.contains(key),
                "the refusal names the variable: {message}"
            );
        }
    }

    /// `assemble` builds an app whose extension names a daemon, and leaves the
    /// process's profile alone: installing it is `run`'s job alone.
    #[test]
    fn assemble_builds_with_a_daemon_profile_and_does_not_install_it() {
        let app = build_app(
            AppExtensions::none()
                .plugin(fixture_plugin())
                .daemon_profile(fixture_daemon_profile()),
        );

        assert!(app.try_state::<FixtureState>().is_some());
        assert_eq!(
            active(),
            &profile_for_this_build(),
            "assemble must not install the extension's profile"
        );
    }
}

#[test]
fn extension_plugin_setup_runs_and_its_state_is_managed() {
    let app = build_app(AppExtensions::none().plugin(fixture_plugin()));

    let state = app
        .try_state::<FixtureState>()
        .expect("the plugin's setup managed its state");
    assert_eq!(state.reply, "pong");
}

#[test]
fn extension_plugin_command_answers_through_ipc_once_granted() {
    let app = build_app_granting(
        AppExtensions::none().plugin(fixture_plugin()),
        "plugin:fixture|fixture_ping",
    );

    assert_eq!(
        invoke(&app, "plugin:fixture|fixture_ping").expect("the granted command answers"),
        "pong"
    );
}

#[test]
fn extension_plugin_command_is_denied_without_a_grant() {
    let app = build_app(AppExtensions::none().plugin(fixture_plugin()));
    assert!(
        app.try_state::<FixtureState>().is_some(),
        "the plugin is registered, so a refusal below is the capability check's"
    );

    let refusal = invoke(&app, "plugin:fixture|fixture_ping")
        .expect_err("an ungranted plugin command is refused");
    let refusal = refusal.to_string();
    assert!(
        refusal.contains("fixture_ping") && refusal.contains("not allowed"),
        "refused for the missing grant on the command, got: {refusal}"
    );
}

#[test]
fn extension_plugin_named_like_a_core_plugin_is_not_registered() {
    let names = core_plugin_names::<MockRuntime>();
    assert!(
        !names.is_empty(),
        "core registers plugins, so there are names to protect"
    );
    for name in names {
        let core_ran = Arc::new(AtomicBool::new(false));
        let extension_ran = Arc::new(AtomicBool::new(false));
        // Stands in for the plugin core registers under this name before it
        // calls `assemble`.
        let builder = mock_builder().plugin(probe_plugin(name, &core_ran));
        assemble(
            builder,
            AppExtensions::none().plugin(probe_plugin(name, &extension_ran)),
        )
        .build(mock_context(noop_assets()))
        .expect("the app builds");

        assert!(
            !extension_ran.load(Ordering::SeqCst),
            "the setup of an extension plugin named {name} must not run"
        );
        assert!(
            core_ran.load(Ordering::SeqCst),
            "the extension plugin named {name} must not replace core's plugin"
        );
    }
}

#[test]
fn second_extension_plugin_with_the_same_name_is_not_registered() {
    let second_ran = Arc::new(AtomicBool::new(false));
    let app = build_app(
        AppExtensions::none()
            .plugin(fixture_plugin())
            .plugin(probe_plugin(FIXTURE, &second_ran)),
    );

    assert!(
        app.try_state::<FixtureState>().is_some(),
        "the first registration stays"
    );
    assert!(
        !second_ran.load(Ordering::SeqCst),
        "the second plugin's setup must not run"
    );
}

#[test]
fn extension_plugins_are_registered_in_the_order_they_were_added() {
    fn recording_plugin(
        name: &'static str,
        order: &Arc<Mutex<Vec<&'static str>>>,
    ) -> TauriPlugin<MockRuntime> {
        let order = Arc::clone(order);
        PluginBuilder::<MockRuntime>::new(name)
            .setup(move |_app, _api| {
                order.lock().unwrap().push(name);
                Ok(())
            })
            .build()
    }

    let order = Arc::new(Mutex::new(Vec::new()));
    build_app(
        AppExtensions::none()
            .plugin(recording_plugin("first", &order))
            .plugin(recording_plugin("second", &order))
            .plugin(recording_plugin("third", &order)),
    );

    assert_eq!(*order.lock().unwrap(), ["first", "second", "third"]);
}

#[test]
fn core_plugin_names_are_unique() {
    // A repeated name would have the later core plugin replace the earlier one.
    let names = core_plugin_names::<MockRuntime>();
    let unique: std::collections::HashSet<_> = names.iter().collect();

    assert_eq!(unique.len(), names.len(), "core plugin names: {names:?}");
}

#[test]
#[cfg(desktop)]
fn single_instance_plugin_is_the_first_core_plugin() {
    // The single-instance plugin must be registered before any other plugin.
    assert_eq!(
        core_plugin_names::<MockRuntime>().first(),
        Some(&tauri_plugin_single_instance::init::<MockRuntime, _>(|_, _, _| {}).name())
    );
}

#[test]
fn none_registers_nothing() {
    for ext in [AppExtensions::none(), AppExtensions::default()] {
        let app = build_app(ext);

        assert!(
            app.try_state::<FixtureState>().is_none(),
            "no plugin ran its setup"
        );
        // Tauri cannot list the plugins an app registered, so only the names
        // this suite could have registered are probed.
        for name in core_plugin_names::<MockRuntime>()
            .into_iter()
            .chain([FIXTURE])
        {
            assert!(
                !app.handle().remove_plugin(name),
                "no plugin named {name} is registered"
            );
        }
    }
}

/// How long a test waits for something that should happen at once before it
/// fails instead of hanging.
const PROMPTLY: Duration = Duration::from_secs(10);

/// A channel that never connects, for tests that do not reach a daemon. Needs
/// a tokio runtime.
async fn idle_channel() -> Channel {
    GrpcClient::connect_lazy().channel().await
}

/// Calls `spawn_daemon_ready_tasks` on a thread of its own and fails the test
/// when it does not return, so a call that waited for a task shows up as a
/// failure and not as a hang.
async fn spawn_tasks(app: &App<MockRuntime>, grpc: GrpcClient, shutdown: &CancellationToken) {
    let handle = app.handle().clone();
    let shutdown = shutdown.clone();
    tokio::time::timeout(
        PROMPTLY,
        tokio::task::spawn_blocking(move || spawn_daemon_ready_tasks(&handle, grpc, &shutdown)),
    )
    .await
    .expect("spawn_daemon_ready_tasks must return without waiting for a task")
    .expect("spawn_daemon_ready_tasks must not panic");
}

/// A channel-rebuilt hook that waits `delay`, then records `name`.
fn recording_hook(
    order: &Arc<Mutex<Vec<&'static str>>>,
    name: &'static str,
    delay: Duration,
) -> impl Fn(AppHandle<MockRuntime>, Channel) -> BoxFuture<'static, ()> + Send + Sync + 'static {
    let order = Arc::clone(order);
    move |_app, _channel| {
        let order = Arc::clone(&order);
        async move {
            tokio::time::sleep(delay).await;
            order.lock().unwrap().push(name);
        }
        .boxed()
    }
}

/// State a daemon-ready task can find through the app handle it is given.
struct AppMarker;

#[tokio::test]
async fn daemon_ready_tasks_are_spawned_not_awaited() {
    let (release, released) = oneshot::channel::<()>();
    let (finish, finished) = oneshot::channel::<()>();
    let app = build_app(
        AppExtensions::none().on_daemon_ready(move |_ready| async move {
            // Parked until the test releases it, which it does only after
            // `spawn_daemon_ready_tasks` has returned.
            released.await.expect("the test releases the task");
            finish.send(()).expect("the test waits for the task");
        }),
    );

    spawn_tasks(&app, GrpcClient::connect_lazy(), &CancellationToken::new()).await;
    release.send(()).expect("the task is waiting");

    tokio::time::timeout(PROMPTLY, finished)
        .await
        .expect("the task ran once it was released")
        .expect("the task finished");
}

#[tokio::test]
async fn daemon_ready_task_gets_the_app_the_client_and_a_child_shutdown_token() {
    let (tx, rx) = oneshot::channel::<(bool, GrpcClient, CancellationToken)>();
    let app = build_app(
        AppExtensions::none().on_daemon_ready(move |ready| async move {
            let has_marker = ready.app.try_state::<AppMarker>().is_some();
            let _ = tx.send((has_marker, ready.grpc, ready.shutdown));
        }),
    );
    app.manage(AppMarker);
    let grpc = GrpcClient::connect_lazy();
    let core_shutdown = CancellationToken::new();

    spawn_tasks(&app, grpc.clone(), &core_shutdown).await;
    let (has_marker, task_grpc, task_shutdown) = tokio::time::timeout(PROMPTLY, rx)
        .await
        .expect("the task ran")
        .expect("the task reported back");

    assert!(has_marker, "the task gets the running app's handle");

    // Both clients share one state, so a switch through one reaches the other.
    let switches = task_grpc.subscribe_active_database();
    grpc.set_active_database(Some("other".to_string())).await;
    assert!(
        switches.has_changed().expect("the sender is alive"),
        "the task's client is a clone of the one core passed"
    );

    assert!(!task_shutdown.is_cancelled());
    core_shutdown.cancel();
    assert!(
        task_shutdown.is_cancelled(),
        "cancelling core's token cancels the task's"
    );
}

#[tokio::test]
async fn daemon_ready_tasks_each_get_their_own_shutdown_token() {
    let (tx, mut rx) = mpsc::unbounded_channel::<CancellationToken>();
    let (first_tx, second_tx) = (tx.clone(), tx);
    let app = build_app(
        AppExtensions::none()
            .on_daemon_ready(move |ready| async move {
                let _ = first_tx.send(ready.shutdown);
            })
            .on_daemon_ready(move |ready| async move {
                let _ = second_tx.send(ready.shutdown);
            }),
    );
    let core_shutdown = CancellationToken::new();

    spawn_tasks(&app, GrpcClient::connect_lazy(), &core_shutdown).await;
    let mut tokens = Vec::new();
    for _ in 0..2 {
        tokens.push(
            tokio::time::timeout(PROMPTLY, rx.recv())
                .await
                .expect("both tasks ran")
                .expect("a task reported its token"),
        );
    }

    tokens[0].cancel();
    assert!(
        !tokens[1].is_cancelled(),
        "cancelling one task's token must not cancel another's"
    );
    assert!(
        !core_shutdown.is_cancelled(),
        "cancelling a task's token must not cancel core's"
    );
}

#[tokio::test]
async fn a_panicking_daemon_ready_task_does_not_stop_the_others() {
    let (tx, rx) = oneshot::channel::<()>();
    let app = build_app(
        AppExtensions::none()
            .on_daemon_ready(|_ready| async move { panic!("a daemon-ready task panics") })
            // Panics before it returns a future at all.
            .on_daemon_ready(|_ready| -> std::future::Ready<()> {
                panic!("a daemon-ready task panics before its first await")
            })
            .on_daemon_ready(move |_ready| async move {
                let _ = tx.send(());
            }),
    );

    spawn_tasks(&app, GrpcClient::connect_lazy(), &CancellationToken::new()).await;

    tokio::time::timeout(PROMPTLY, rx)
        .await
        .expect("the task after the panicking ones ran")
        .expect("the task reported back");
}

#[tokio::test]
async fn daemon_ready_tasks_run_once() {
    let runs = Arc::new(AtomicUsize::new(0));
    let (done, mut finished) = mpsc::unbounded_channel::<()>();
    let task = |runs: &Arc<AtomicUsize>, done: &mpsc::UnboundedSender<()>| {
        let (runs, done) = (Arc::clone(runs), done.clone());
        move |_ready| async move {
            runs.fetch_add(1, Ordering::SeqCst);
            let _ = done.send(());
        }
    };
    let app = build_app(
        AppExtensions::none()
            .on_daemon_ready(task(&runs, &done))
            .on_daemon_ready(task(&runs, &done)),
    );
    let grpc = GrpcClient::connect_lazy();
    let core_shutdown = CancellationToken::new();

    spawn_tasks(&app, grpc.clone(), &core_shutdown).await;
    // A second call, such as one from a second window, runs nothing.
    spawn_tasks(&app, grpc, &core_shutdown).await;

    for _ in 0..2 {
        tokio::time::timeout(PROMPTLY, finished.recv())
            .await
            .expect("each task ran")
            .expect("a task reported back");
    }
    assert_eq!(runs.load(Ordering::SeqCst), 2, "each task ran exactly once");
    assert!(finished.try_recv().is_err(), "no task ran a second time");
}

#[tokio::test(start_paused = true)]
async fn channel_rebuilt_hooks_run_in_registration_order() {
    let order = Arc::new(Mutex::new(Vec::new()));
    // The first hook is the slowest, so hooks that ran side by side would
    // finish in the opposite order.
    let app = build_app(
        AppExtensions::none()
            .on_channel_rebuilt(recording_hook(&order, "first", Duration::from_millis(100)))
            .on_channel_rebuilt(recording_hook(&order, "second", Duration::from_millis(10)))
            .on_channel_rebuilt(recording_hook(&order, "third", Duration::ZERO)),
    );

    run_channel_rebuilt_hooks(app.handle(), idle_channel().await).await;

    assert_eq!(*order.lock().unwrap(), ["first", "second", "third"]);
}

#[tokio::test(start_paused = true)]
async fn a_hanging_channel_rebuilt_hook_times_out_and_the_next_still_runs() {
    /// Records that the future holding it was dropped.
    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let hung_future_dropped = Arc::new(AtomicBool::new(false));
    let next_ran = Arc::new(AtomicBool::new(false));
    let app = build_app(
        AppExtensions::none()
            .on_channel_rebuilt({
                let dropped = Arc::clone(&hung_future_dropped);
                move |_app, _channel| {
                    let guard = DropFlag(Arc::clone(&dropped));
                    async move {
                        let _guard = guard;
                        std::future::pending::<()>().await;
                    }
                }
            })
            .on_channel_rebuilt({
                let next_ran = Arc::clone(&next_ran);
                move |_app, _channel| {
                    let next_ran = Arc::clone(&next_ran);
                    async move { next_ran.store(true, Ordering::SeqCst) }
                }
            }),
    );
    let channel = idle_channel().await;

    let started = tokio::time::Instant::now();
    // The outer bound turns a runner that never gives up on the hanging hook
    // into a failure, since the paused clock would otherwise never move on.
    tokio::time::timeout(
        CHANNEL_REBUILT_HOOK_TIMEOUT * 10,
        run_channel_rebuilt_hooks(app.handle(), channel),
    )
    .await
    .expect("a hanging hook must not hold recovery up beyond its timeout");
    let waited = started.elapsed();

    assert!(
        waited >= CHANNEL_REBUILT_HOOK_TIMEOUT,
        "the hook gets its whole timeout, waited {waited:?}"
    );
    assert!(
        waited < CHANNEL_REBUILT_HOOK_TIMEOUT * 2,
        "recovery waits for the hanging hook once, waited {waited:?}"
    );
    assert!(
        next_ran.load(Ordering::SeqCst),
        "the hook after the hanging one still runs"
    );
    // The aborted task is dropped the next time the runtime schedules it.
    tokio::task::yield_now().await;
    assert!(
        hung_future_dropped.load(Ordering::SeqCst),
        "the hanging hook is aborted, not left running"
    );
}

#[tokio::test]
async fn a_panicking_channel_rebuilt_hook_does_not_stop_recovery() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let app = build_app(
        AppExtensions::none()
            .on_channel_rebuilt(|_app, _channel| async move { panic!("a hook panics") })
            // Panics before it returns a future at all.
            .on_channel_rebuilt(|_app, _channel| -> std::future::Ready<()> {
                panic!("a hook panics before its first await")
            })
            .on_channel_rebuilt(recording_hook(&order, "after", Duration::ZERO)),
    );

    run_channel_rebuilt_hooks(app.handle(), idle_channel().await).await;

    assert_eq!(
        *order.lock().unwrap(),
        ["after"],
        "the hook after the panicking ones still runs"
    );
}

#[tokio::test]
async fn hooks_are_a_no_op_without_extension_state() {
    // `mock_app` is not built through `assemble`, so it has no hook state.
    let app = mock_app();

    run_channel_rebuilt_hooks(app.handle(), idle_channel().await).await;
    spawn_tasks(&app, GrpcClient::connect_lazy(), &CancellationToken::new()).await;
}

/// An update source that reads a version endpoint at `url` and downloads from
/// `download_url`.
fn endpoint_source(url: &'static str, download_url: Option<&'static str>) -> UpdateSource {
    UpdateSource {
        latest: LatestVersionSource::VersionEndpoint { url },
        download_url,
    }
}

#[test]
fn none_manages_the_builtin_update_source() {
    let app = build_app(AppExtensions::none());

    let managed = app
        .try_state::<UpdateSourceState>()
        .expect("assemble manages an update source even when the extension sets none");
    assert_eq!(managed.0, builtin_update_source());
    assert_eq!(update_source_for_app(app.handle()), builtin_update_source());
}

#[test]
fn an_extension_update_source_replaces_the_builtin() {
    let source = endpoint_source(
        "https://updates.example.test/latest",
        Some("https://example.test/download"),
    );
    assert_ne!(
        source,
        builtin_update_source(),
        "the fixture source must differ from the built-in one to show it replaced it"
    );

    let app = build_app(AppExtensions::none().update_source(source.clone()));

    assert_eq!(update_source_for_app(app.handle()), source);
}

#[test]
fn the_last_update_source_wins() {
    let first = endpoint_source("https://first.example.test/latest", None);
    let last = endpoint_source(
        "https://last.example.test/latest",
        Some("https://last.example.test/download"),
    );

    let app = build_app(
        AppExtensions::none()
            .update_source(first)
            .update_source(last.clone()),
    );

    assert_eq!(update_source_for_app(app.handle()), last);
}

#[test]
fn update_source_falls_back_to_the_builtin_without_extension_state() {
    // `mock_app` is not built through `assemble`, so it manages no source.
    let app = mock_app();

    assert_eq!(update_source_for_app(app.handle()), builtin_update_source());
}

/// Answers one HTTP request on a local port with `body` as JSON, and returns
/// the URL it serves, leaked because an update source holds `&'static str`.
async fn serve_json_once(body: &'static str) -> &'static str {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port is free");
    let url = format!(
        "http://{}/latest",
        listener.local_addr().expect("a bound address")
    );
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("the check connects");
        // Read the whole request head first: closing with unread bytes would
        // reset the connection before the client reads the answer.
        let mut request = Vec::new();
        let mut chunk = [0u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = socket.read(&mut chunk).await.expect("the request arrives");
            assert!(read > 0, "the client closed before finishing its request");
            request.extend_from_slice(&chunk[..read]);
        }
        let answer = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n{body}",
            body.len()
        );
        socket
            .write_all(answer.as_bytes())
            .await
            .expect("the answer is sent");
    });
    Box::leak(url.into_boxed_str())
}

#[tokio::test]
async fn an_extension_update_source_decides_the_version_and_the_download() {
    let url = serve_json_once(r#"{"version":"v99.0.0"}"#).await;
    let app = build_app(
        AppExtensions::none()
            .update_source(endpoint_source(url, Some("https://example.test/download"))),
    );

    let status = tokio::time::timeout(PROMPTLY, check_for_update_for_app(app.handle()))
        .await
        .expect("the check finishes");

    // The mock context's bundle version.
    assert_eq!(status.current, "0.1.0");
    assert_eq!(
        status.latest.as_deref(),
        Some("v99.0.0"),
        "no answer from the local endpoint (is HTTP_PROXY or ALL_PROXY set without \
         NO_PROXY covering 127.0.0.1?)"
    );
    assert!(status.update_available);
    assert_eq!(
        status.download_url.as_deref(),
        Some("https://example.test/download")
    );
}
