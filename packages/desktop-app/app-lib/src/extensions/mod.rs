//! Extension points for an app crate built on this library.
//!
//! An app crate outside core can depend on this library and contribute to the
//! desktop app it builds. Everything an extension contributes to Tauri arrives
//! as a [Tauri plugin](tauri::plugin::Plugin): commands, managed state, setup
//! and run-event handling all travel inside it. A `tauri::Builder` holds only
//! one invoke handler and one setup closure, and a second call replaces the
//! first, so extension commands cannot be entries in core's own command list.
//!
//! [`AppExtensions`] collects what an extension contributes and [`assemble`]
//! applies it to a `tauri::Builder`. An app with no extension, built from
//! [`AppExtensions::none`], behaves exactly as core alone does.
//!
//! Besides plugins, an extension can run work once the daemon is up
//! ([`AppExtensions::on_daemon_ready`]) and react when the shared gRPC channel
//! is rebuilt ([`AppExtensions::on_channel_rebuilt`]). Both ride the channel
//! core owns, so an extension never dials the daemon socket itself.
//!
//! An extension can also decide where the app's update check looks and what
//! the update banner's Download button opens ([`AppExtensions::update_source`]),
//! and which daemon the app installs and starts
//! ([`AppExtensions::daemon_profile`]). `run` installs that profile before it
//! builds the app, so it is the one part of an extension `assemble` ignores.

use std::collections::HashSet;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use tauri::plugin::Plugin;
use tauri::{AppHandle, Builder, Manager, Runtime};

use crate::services::GrpcClient;
use crate::update_check::{builtin_update_source, UpdateSourceState};

pub use crate::daemon_profile::DaemonProfile;
pub use crate::update_check::{LatestVersionSource, UpdateSource};
pub use tokio_util::sync::CancellationToken;
pub use tonic::transport::Channel;

#[cfg(test)]
mod fixture_tests;

/// The version of the extension API, as `(major, minor)`.
///
/// A breaking change to the API bumps the major number and an additive change
/// bumps the minor number. The TypeScript host API carries the same value, and
/// the two must stay equal.
pub const EXTENSION_API_VERSION: (u32, u32) = (2, 3);

/// Core's own Tauri plugins, in the order they are registered.
///
/// This is the only list of them. [`register_core_plugins`] registers it for
/// `run`, and [`core_plugin_names`] reads the names off it for [`assemble`],
/// which skips an extension plugin that reuses one. A plugin registered under
/// an existing name replaces the earlier one without a warning, so an
/// extension plugin with a core plugin's name would replace core's. `run`
/// registers no plugin of its own, so a plugin added here is protected the
/// moment it exists, with no second list to update.
///
/// The single-instance plugin comes first, because it must be registered
/// before any other plugin.
///
/// Building the list must have no effects: [`core_plugin_names`] builds it to
/// read the names and [`register_core_plugins`] builds it again to register
/// them, so a plugin's constructor only describes the plugin. Anything with an
/// effect (opening a file, starting a thread, claiming a resource) belongs in
/// its setup hook, which runs once when the app is built.
pub(crate) fn core_plugins<R: Runtime>() -> Vec<Box<dyn Plugin<R>>> {
    let mut plugins: Vec<Box<dyn Plugin<R>>> = Vec::new();

    // The daemon's tray always spawns a fresh UI process when a database is
    // picked (see `nodespace_daemon::tray::TrayState::open_ui`) — it has no
    // reliable way to know whether a UI process from an earlier launch is still
    // alive, and none at all when the app was started outside the tray. Rather
    // than the daemon tracking liveness, this plugin makes any second launch
    // detect the running instance, forward its argv to it, and exit
    // immediately, so a real UI process almost never survives alongside an
    // existing one regardless of launch path. Note this plugin's macOS backend
    // claims the lock with a connect-then-bind probe rather than one atomic OS
    // primitive (unlike its Windows/Linux backends), so it is not an absolute
    // guarantee there — see the doc comment on `TrayState::open_ui` in the
    // daemon crate.
    #[cfg(desktop)]
    plugins.push(Box::new(tauri_plugin_single_instance::init::<R, _>(
        |app, argv, _cwd| {
            crate::handle_relaunch(app, &argv);
        },
    )));

    plugins.push(Box::new(tauri_plugin_opener::init::<R>()));
    plugins.push(Box::new(tauri_plugin_dialog::init::<R>()));
    plugins
}

/// The names of [`core_plugins`], in registration order.
pub(crate) fn core_plugin_names<R: Runtime>() -> Vec<&'static str> {
    core_plugins::<R>()
        .iter()
        .map(|plugin| plugin.name())
        .collect()
}

/// Registers [`core_plugins`] on `builder`, in order.
pub(crate) fn register_core_plugins<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    core_plugins::<R>()
        .into_iter()
        .fold(builder, |builder, plugin| builder.plugin_boxed(plugin))
}

/// What an extension contributes to the desktop app.
///
/// Build one with [`AppExtensions::none`] and add contributions with the
/// methods below, then hand it to [`assemble`]. The runtime parameter is
/// `tauri::Wry` unless a test picks another runtime.
pub struct AppExtensions<R: Runtime = tauri::Wry> {
    plugins: Vec<Box<dyn Plugin<R>>>,
    daemon_ready: Vec<DaemonReadyTask<R>>,
    channel_rebuilt: Vec<ChannelRebuiltHook<R>>,
    update_source: Option<UpdateSource>,
    daemon_profile: Option<DaemonProfile>,
}

/// What a daemon-ready task receives when core starts it.
///
/// Everything in it is cheap to clone and owned, so a task can move it into
/// whatever it spawns.
pub struct DaemonReady<R: Runtime = tauri::Wry> {
    /// The running app.
    pub app: AppHandle<R>,
    /// A clone of the gRPC client core manages. Take services from
    /// [`GrpcClient::channel`] rather than dialing the socket (see
    /// [`AppExtensions::on_daemon_ready`]).
    pub grpc: GrpcClient,
    /// Cancelled when core shuts down. It is this task's own child of core's
    /// shutdown token, so cancelling it affects neither core nor another task.
    pub shutdown: CancellationToken,
}

type DaemonReadyTask<R> = Box<dyn FnOnce(DaemonReady<R>) -> BoxFuture<'static, ()> + Send>;
type ChannelRebuiltHook<R> =
    Arc<dyn Fn(AppHandle<R>, Channel) -> BoxFuture<'static, ()> + Send + Sync>;

/// How long each [`AppExtensions::on_channel_rebuilt`] hook may run before core
/// gives up on it and moves on.
pub const CHANNEL_REBUILT_HOOK_TIMEOUT: Duration = Duration::from_secs(2);

impl<R: Runtime> AppExtensions<R> {
    /// An extension that contributes nothing.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: Vec::new(),
            daemon_ready: Vec::new(),
            channel_rebuilt: Vec::new(),
            update_source: None,
            daemon_profile: None,
        }
    }

    /// Adds a Tauri plugin. Plugins are registered in the order they were added.
    ///
    /// A plugin's setup runs before core's own setup, so it must not assume the
    /// daemon connection, the shutdown token or a running daemon exist yet.
    #[must_use]
    pub fn plugin<P: Plugin<R> + 'static>(mut self, plugin: P) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    /// Runs `task` once per process, after core has made its attempt to start
    /// the daemon.
    ///
    /// Core spawns the task from its startup task, once the daemon start
    /// attempt has finished and core's node watcher and token stream are
    /// wired. It never awaits the task, so a slow or hanging task delays
    /// nothing. Tasks are spawned in the order they were added, each as its
    /// own task, but nothing orders when they start running. A task that
    /// panics is logged without affecting the others.
    ///
    /// # What the task must tolerate
    ///
    /// * **The daemon may be unreachable.** The start attempt can fail, and the
    ///   daemon can stop at any time afterwards. A task must treat a failed
    ///   call as an expected state and retry later, never as a reason to
    ///   panic.
    /// * **Use the shared channel.** A task makes its calls on
    ///   [`GrpcClient::channel`] from [`DaemonReady::grpc`] and never dials the
    ///   socket itself. A parallel channel to the same socket has failed with
    ///   "Service was not ready: transport error" once a stream was dropped.
    /// * **Channels get rebuilt.** A task that caches a client built on that
    ///   channel, or holds a stream open on it, keeps using the old
    ///   connection after a rebuild. It registers
    ///   [`AppExtensions::on_channel_rebuilt`] to pick up the new one, and it
    ///   reads the channel and stores its client while holding the same lock
    ///   the hook takes to replace that client. Core swaps the channel before
    ///   it runs any hook, so the task then either reads the new channel or
    ///   stores its client before the hook replaces it. Without the lock, a
    ///   task that read the old channel can store its client after the hook
    ///   ran and stay on the dead connection.
    /// * **Shutdown arrives through [`DaemonReady::shutdown`].** A task that
    ///   loops or holds a resource watches that token and returns when it is
    ///   cancelled.
    #[must_use]
    pub fn on_daemon_ready<F, Fut>(mut self, task: F) -> Self
    where
        F: FnOnce(DaemonReady<R>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.daemon_ready
            .push(Box::new(move |ready| task(ready).boxed()));
        self
    }

    /// Runs `hook` each time core rebuilds the shared gRPC channel, with the
    /// new channel.
    ///
    /// Core rebuilds the channel when it finds the connection wedged: the
    /// daemon answers a freshly dialed client while the long-lived channel
    /// hangs. Core then runs every hook, one after another in the order they
    /// were added, and only then probes the new channel again. A caller of the
    /// recovery therefore sees the hooks' work done before it learns the
    /// channel is healthy.
    ///
    /// Each hook is bounded by [`CHANNEL_REBUILT_HOOK_TIMEOUT`], because the
    /// frontend waits on the recovery. A hook that runs past it is aborted and
    /// logged, and a hook that panics is logged, and in both cases recovery
    /// goes on with the next hook. A hook that has more to do than the timeout
    /// allows spawns it and returns.
    ///
    /// # What a hook is for
    ///
    /// A channel handed out before the rebuild still points at the old
    /// connection. Anything an extension built on one of those, such as a
    /// service client it caches or a stream it holds open, keeps riding the
    /// dead connection until the extension replaces it, and this hook is where
    /// it does. An extension that takes a fresh channel from
    /// [`GrpcClient::channel`] on every call needs no hook.
    ///
    /// The daemon may be unreachable when the hook runs, and the hook must
    /// tolerate that the way a daemon-ready task does (see
    /// [`AppExtensions::on_daemon_ready`]).
    #[must_use]
    pub fn on_channel_rebuilt<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn(AppHandle<R>, Channel) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.channel_rebuilt
            .push(Arc::new(move |app, channel| hook(app, channel).boxed()));
        self
    }

    /// Sets where the app's update check looks for the latest version and
    /// what the update banner's Download button opens. If it is called more
    /// than once, the last call wins. Without it the app uses core's built-in
    /// source, which in a default build is [`UpdateSource::community`].
    ///
    /// * **Fixed for the process.** [`assemble`] stores the source when the app
    ///   is built. The check at startup and every on-demand check from the
    ///   frontend read that one source, and nothing changes it afterwards.
    /// * **The running version is the app's own.** The check compares the
    ///   latest version with the version in the app's bundle config
    ///   (`tauri.conf.json`), so an app crate that ships its own version
    ///   numbers sets them there.
    /// * **A version endpoint's shape is fixed.** A
    ///   [`LatestVersionSource::VersionEndpoint`] must answer a GET with JSON
    ///   carrying a `version` string, as semver with or without a leading `v`:
    ///   `{"version": "1.4.0"}` or `{"version": "v1.4.0"}`. Any other answer
    ///   means "no update known", and the banner does not appear.
    /// * **`download_url: None` hides Download.** The banner then shows the
    ///   update without a way to fetch it, so a source that names no page
    ///   leaves getting the update to the app crate. A page it does name must
    ///   be an `http` or `https` URL, the only schemes the banner can open.
    #[must_use]
    pub fn update_source(mut self, source: UpdateSource) -> Self {
        self.update_source = Some(source);
        self
    }

    /// Names the daemon this app installs, registers and starts, in place of
    /// the build's default profile, [`DaemonProfile::community`]. If it is
    /// called more than once, the last call wins.
    ///
    /// `run` installs the profile before it creates the app, and the profile
    /// then stays fixed for the life of the process. [`assemble`] ignores it.
    ///
    /// What each field controls:
    ///
    /// * **`binary_name`**: the daemon binary the app installs, registers and
    ///   starts, and the image name it kills on Windows. At startup the app
    ///   also asks the daemon answering on the socket which executable it runs,
    ///   and boots out the registration when that is not this binary.
    /// * **`service_env`**: extra environment in the daemon's launchd
    ///   registration, after core's own variables. The systemd unit and the
    ///   Windows spawn do not carry it.
    ///
    /// # What a profile cannot change
    ///
    /// The launchd label, the socket, the single-instance lock, the UI pid
    /// file and the incompatible-database marker are core's shared service
    /// identity (ADR-084 §4). Every app built on core uses the same ones, and
    /// no profile configures them.
    ///
    /// The daemon `binary_name` names must therefore honour the registration
    /// contract of ADR-084 §4.2: it accepts the arguments and environment
    /// core's launcher passes, uses core's default names for the socket, lock,
    /// UI pid file and marker, gives its exit status the meanings the contract
    /// defines, takes core's single-instance lock, and shows core's tray when
    /// passed `--tray`.
    ///
    /// # Panics
    ///
    /// If a `service_env` key is one of the variables core's launcher sets
    /// itself, `NODESPACED_SOCKET` or `NODESPACE_UI_BINARY`. launchd accepts
    /// the repeated key, and the profile's value, which comes later, would
    /// silently replace core's.
    #[must_use]
    pub fn daemon_profile(mut self, profile: DaemonProfile) -> Self {
        if let Some(key) = profile.reserved_service_env_key() {
            panic!(
                "DaemonProfile::service_env sets `{key}`, which core's launcher sets itself; \
                 a profile may only add variables of its own"
            );
        }
        self.daemon_profile = Some(profile);
        self
    }

    /// Takes the daemon profile for `run` to install, leaving none.
    pub(crate) fn take_daemon_profile(&mut self) -> Option<DaemonProfile> {
        self.daemon_profile.take()
    }
}

// Not derived: `#[derive(Default)]` would require `R: Default`, and a runtime
// type has no reason to implement it.
impl<R: Runtime> Default for AppExtensions<R> {
    fn default() -> Self {
        Self::none()
    }
}

/// Applies an extension's contributions to a `tauri::Builder`.
///
/// This is the extension half of building the app. It registers each extension
/// plugin, in the order it was added, and manages the daemon-ready tasks and
/// channel-rebuilt hooks where core's startup and channel recovery find them
/// (`ExtensionHooks`, which is managed even when there are none). It also
/// manages the update source every update check reads: the extension's, or
/// core's built-in one when the extension sets none. Core's own plugins,
/// commands and setup are applied by the desktop entry point, `run`, around
/// this call.
///
/// Call it once per builder: the hook state and the update source are managed
/// state, and Tauri refuses to manage one type twice.
///
/// # Plugins
///
/// * The caller registers core's own plugins on `builder` first, with the
///   single-instance plugin before any other, as that plugin requires. `run`
///   does this. `assemble` registers the extension plugins after them.
/// * A plugin's setup runs inside `Builder::build`, before core's setup.
/// * A plugin's commands are invoked from the frontend as
///   `plugin:<name>|<command>`. Tauri always checks them against the
///   capabilities, so the app crate that owns the capability files must grant
///   them there. Core's capabilities never name an extension's commands.
/// * A plugin whose name is one of core's own plugins, or that reuses the name
///   of an earlier extension plugin, is not registered and is logged as an
///   error. Tauri would otherwise let the later plugin silently replace the
///   earlier one. The first registration wins.
///
/// # Why this is generic over the runtime
///
/// Core's own commands take the concrete `tauri::Wry` handle types, and core's
/// setup starts the real daemon, so neither can run on a mock runtime.
/// `assemble` touches neither, which lets tests drive everything an extension
/// contributes through `tauri::test::MockRuntime`.
#[must_use]
pub fn assemble<R: Runtime>(mut builder: Builder<R>, extensions: AppExtensions<R>) -> Builder<R> {
    let core_names = core_plugin_names::<R>();
    let mut registered: HashSet<&'static str> = HashSet::new();
    for plugin in extensions.plugins {
        let name = plugin.name();
        if core_names.contains(&name) {
            tracing::error!(
                plugin = name,
                "extension plugin skipped: its name belongs to a plugin core registers"
            );
            continue;
        }
        if !registered.insert(name) {
            tracing::error!(
                plugin = name,
                "extension plugin skipped: an earlier extension plugin already uses its name"
            );
            continue;
        }
        builder = builder.plugin_boxed(plugin);
    }
    builder = builder.manage(UpdateSourceState(
        extensions
            .update_source
            .unwrap_or_else(builtin_update_source),
    ));
    builder.manage(ExtensionHooks {
        daemon_ready: Mutex::new(extensions.daemon_ready),
        channel_rebuilt: extensions.channel_rebuilt,
    })
}

/// The daemon-ready tasks and channel-rebuilt hooks of the assembled app, in
/// managed state for core's startup and channel recovery to find.
pub(crate) struct ExtensionHooks<R: Runtime> {
    /// Taken by the first [`spawn_daemon_ready_tasks`], so a task runs once per
    /// process.
    daemon_ready: Mutex<Vec<DaemonReadyTask<R>>>,
    channel_rebuilt: Vec<ChannelRebuiltHook<R>>,
}

/// Spawns every daemon-ready task, and returns without waiting for any of them.
///
/// Core's startup task calls this once the daemon start attempt has finished
/// and the watcher and token stream are wired. Each task runs on Tauri's async
/// runtime with `grpc`, a clone of the app handle, and its own child of
/// `shutdown`. The tasks are taken, so a second call spawns nothing. A task
/// that panics is logged and the others are unaffected. An app that was not
/// built with [`assemble`] has no tasks, and this does nothing.
///
/// Not part of the extension API: only core's startup calls it, and the seam
/// tests.
#[doc(hidden)]
pub fn spawn_daemon_ready_tasks<R: Runtime>(
    app: &AppHandle<R>,
    grpc: GrpcClient,
    shutdown: &CancellationToken,
) {
    let Some(hooks) = app.try_state::<ExtensionHooks<R>>() else {
        return;
    };
    let tasks = std::mem::take(
        &mut *hooks
            .daemon_ready
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
    );
    for task in tasks {
        let ready = DaemonReady {
            app: app.clone(),
            grpc: grpc.clone(),
            shutdown: shutdown.child_token(),
        };
        tauri::async_runtime::spawn(async move {
            // The call to `task` is inside the guarded future too, so a task
            // that panics before it returns its future is caught the same way.
            let outcome = AssertUnwindSafe(async move { task(ready).await })
                .catch_unwind()
                .await;
            if let Err(panic) = outcome {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("(the panic payload is not a string)");
                tracing::error!(panic = message, "a daemon-ready task panicked");
            }
        });
    }
}

/// Runs every channel-rebuilt hook with `channel`, one after another in the
/// order they were added, and returns when the last has finished.
///
/// Core's channel recovery calls this after it rebuilds the channel and before
/// it probes the new one. Each hook runs as its own spawned task under
/// [`CHANNEL_REBUILT_HOOK_TIMEOUT`]. A hook that times out is aborted and
/// logged, a hook that panics is logged, and the next hook runs either way. An
/// app that was not built with [`assemble`] has no hooks, and this does
/// nothing.
///
/// Call it from inside a tokio runtime. Not part of the extension API: only
/// core's channel recovery calls it, and the seam tests.
#[doc(hidden)]
pub async fn run_channel_rebuilt_hooks<R: Runtime>(app: &AppHandle<R>, channel: Channel) {
    let Some(hooks) = app
        .try_state::<ExtensionHooks<R>>()
        .map(|state| state.channel_rebuilt.clone())
    else {
        return;
    };

    for hook in hooks {
        let (app, channel) = (app.clone(), channel.clone());
        // The hook is called inside the spawned task, so one that panics before
        // it returns its future is contained like one that panics later.
        let mut task = tokio::spawn(async move { hook(app, channel).await });
        match tokio::time::timeout(CHANNEL_REBUILT_HOOK_TIMEOUT, &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::error!(%error, "a channel-rebuilt hook failed; recovery continues");
            }
            Err(_elapsed) => {
                task.abort();
                tracing::warn!(
                    timeout = ?CHANNEL_REBUILT_HOOK_TIMEOUT,
                    "a channel-rebuilt hook timed out and was aborted; recovery continues"
                );
            }
        }
    }
}
