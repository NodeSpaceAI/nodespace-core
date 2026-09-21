//! `nodespaced` — background daemon that owns the SQLite database lock and serves
//! NodeSpace operations over gRPC on a Unix Domain Socket.
//!
//! Lifecycle:
//!   1. Initialize tracing.
//!   2. Install signal handlers (fail-fast — a daemon that can't observe
//!      shutdown signals is broken).
//!   3. Open `SqliteStore` (embedded SQLite/libsql) at the configured path.
//!   4. Build `NodeService` from `nodespace-core`.
//!   5. Bring up the system tray on the main thread and spawn the tonic
//!      `NodeService` handler on a worker tokio runtime.
//!   6. Tear down cleanly on `SIGTERM`, `SIGINT`, or "Quit" from the tray.
//!
//! # Trust model (ADR-052)
//!
//! This daemon is single-user-local and has **no in-core authorization layer**
//! — no `user_id`, no actor concept, no per-request ACL. Every request that
//! reaches the gRPC services below is served with the daemon owner's full
//! authority over the entire knowledge graph. That is deliberate, not an
//! omission to be backfilled: for a single-user-local desktop app the **OS
//! socket/pipe permission is the entire authorization boundary**. Whoever can
//! open the socket gets the whole graph.
//!
//! On Unix, [`bind_uds_owner_only`] enforces that boundary as `0o600` with no
//! ambient-umask exposure window. There is currently no peer-credential check
//! (`SO_PEERCRED` / `LOCAL_PEERCRED`) backstopping the file permission — every
//! connection accepted by the listener is handed straight to the tonic
//! server. On Windows, [`create_owner_only_pipe`] enforces the equivalent
//! boundary: the Named Pipe is created with a DACL restricted to the current
//! user's SID (see its doc comment for the exact SDDL and how it was
//! verified on real hardware), and the instance that claims the pipe name
//! uses `first_pipe_instance(true)` so startup fails loudly rather than
//! silently serving on a name another local process pre-created/squatted.
//! There is no equivalent of a peer-credential check on Windows either —
//! every connection accepted by the listener is handed straight to the
//! tonic server. Do not assume a peer-identity check exists anywhere in this
//! file.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use nodespace_agent::local_agent::otlp_tracer;
use nodespace_daemon::tray::layer::TrayMetricsLayer;
use nodespace_daemon::{
    build_base_router, build_shared_services, create_dir_owner_only, resolve_db_path, tray,
    BaseServices, DatabaseManager, DatabaseServiceImpl, DatabaseServices, DbManagerLayer,
    SharedContext,
};
use nodespace_nlp_engine::EmbeddingService;
use tokio::sync::watch;
use tonic::transport::Server;

/// ADR-053: construct the [`DatabaseManager`], register + lazily open the
/// default database, and return the manager together with the default's shared
/// service set. The serve loops clone the per-database impls out of the returned
/// set into [`BaseServices`] and install [`DbManagerLayer`] so requests carrying
/// an `x-ns-database-id` header can reach other registered databases while
/// header-less requests keep hitting the default.
///
/// Opening the default through the manager (rather than building it directly)
/// means the *same* cached service set backs both header-less requests and
/// requests that name the default id explicitly — the file is never opened
/// twice.
/// Keep the tray's Databases submenu in step with the registry for the life of
/// the daemon.
///
/// Pushes the current registry once, then again on every registry or open-set
/// change. Without the follow-ups the submenu shows the registry exactly as it
/// was at daemon boot, so a database created, renamed or removed afterwards — or
/// opened by a switch, or closed by the idle reaper — reads wrong until the
/// daemon restarts.
///
/// The snapshot is re-read after each wake rather than carried on the channel,
/// so a burst of changes collapses into a single refresh.
fn spawn_tray_database_sync(controller: tray::TrayController, manager: Arc<DatabaseManager>) {
    tokio::spawn(async move {
        let mut changes = manager.subscribe_changes();
        controller.databases_changed(manager.list().await);
        while changes.changed().await.is_ok() {
            controller.databases_changed(manager.list().await);
        }
    });
}

async fn open_default_database(
    db_path: &std::path::Path,
    context: SharedContext,
) -> Result<(Arc<DatabaseManager>, Arc<DatabaseServices>)> {
    let manager =
        Arc::new(DatabaseManager::load(DatabaseManager::default_registry_path()?, context).await?);
    // Guard against a registry whose default was seeded with a throwaway temp
    // path (e.g. a test/dev run that redirected the database but not the home
    // dir): the OS purges temp dirs, so serving one silently loses user data.
    // Re-points the default to the standard path before we open it.
    manager.repair_doomed_default(db_path).await?;
    let default_id = manager
        .ensure_default_registered("Default".to_string(), db_path.to_path_buf())
        .await?;
    let bundle = manager.get_or_open(&default_id).await?;
    // Log the path the registry actually resolved the default to — not the
    // boot-time `db_path`, which the registry can and does override.
    if let Some(served) = manager.default_database_path().await {
        tracing::info!(served_db_path = %served.display(), "serving default database");
    }
    Ok((manager, bundle))
}

/// True when this daemon was built as the Pro edition. The sibling
/// `nodespace-sync` repo compiles `nodespaced-pro` with `--features pro`; a
/// community build leaves it off. Same discriminator `edition()` reports.
fn is_pro_build() -> bool {
    cfg!(feature = "pro")
}

/// The socket this daemon binds.
///
/// `NODESPACED_SOCKET` overrides it, but the fallback must resolve to the same
/// build-variant-scoped path the desktop app dials — see
/// `nodespace_proto::socket`. A daemon that fell back to the unscoped
/// `daemon.sock` would serve an endpoint no Pro or dev app ever looks at, while
/// that app reports the daemon as not running.
#[cfg(unix)]
fn socket_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var(nodespace_proto::socket::SOCKET_ENV_VAR) {
        return std::path::PathBuf::from(p);
    }
    default_socket_path_for(cfg!(debug_assertions), is_pro_build())
}

/// The socket [`socket_path`] falls back to when `NODESPACED_SOCKET` is absent,
/// for an arbitrary build variant rather than this binary's own.
///
/// Takes the variant as parameters because a compiled daemon is only ever one
/// variant, so this is the only way an ordinary `#[test]` can check all four —
/// which is exactly what the app/daemon agreement test needs. It reads no
/// `NODESPACED_SOCKET` for the same reason its app-side counterpart doesn't:
/// `cargo test` shares one process, so an env-reading resolver cannot be
/// asserted on without racing every other test that touches that variable.
#[cfg(unix)]
fn default_socket_path_for(is_debug: bool, is_pro: bool) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(home).join(nodespace_proto::socket::daemon_socket_relative(
        is_debug, is_pro,
    ))
}

/// Binds a Unix domain socket so no other local user can ever reach it,
/// without mutating any process-global state (ADR-052 §3: the socket mode is
/// the entire local-authorization boundary for the gRPC surface below).
///
/// `UnixListener::bind` creates the socket file honoring the ambient umask, so
/// a plain `bind`-then-`chmod` leaves it briefly at a wider mode. The tempting
/// fix — narrowing the umask around the bind — is not usable here: `umask(2)`
/// is process-global rather than per-thread, so mutating it silently re-modes
/// every file and directory any *other* thread creates while it is narrowed.
/// This bind happens with a multi-threaded Tokio runtime already live (shared
/// services and their background tasks are built first), and `cargo test`
/// runs every test in one process on a thread pool, so that race is real both
/// in production and, more visibly, in the test binary — a umask-narrowing
/// bind test here previously corrupted directories concurrently created by
/// unrelated tests, failing them with a bogus `Permission denied` on a path
/// they had just created themselves.
///
/// The containing directory carries the guarantee instead: callers restrict it
/// to owner-only before binding (`create_dir_owner_only`), so nobody else can
/// traverse into it and the window between `bind` and `chmod` below is
/// unreachable. Because that precondition is the entire basis of the
/// guarantee, it is enforced here fail-closed rather than assumed.
#[cfg(unix)]
fn bind_uds_owner_only(sock: &std::path::Path) -> Result<tokio::net::UnixListener> {
    use std::os::unix::fs::PermissionsExt;

    let dir = sock
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .with_context(|| format!("socket path {} has no directory", sock.display()))?;
    let dir_mode = std::fs::metadata(dir)
        .with_context(|| format!("Failed to stat socket directory: {}", dir.display()))?
        .permissions()
        .mode()
        & 0o777;
    anyhow::ensure!(
        dir_mode & 0o077 == 0,
        "refusing to bind {}: its directory {} is mode {dir_mode:o} — group/other can reach \
         the socket during the window between bind and chmod",
        sock.display(),
        dir.display()
    );

    let listener = tokio::net::UnixListener::bind(sock)
        .with_context(|| format!("Failed to bind Unix socket: {}", sock.display()))?;
    std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("Failed to set socket permissions: {}", sock.display()))?;
    Ok(listener)
}

#[cfg(windows)]
fn pipe_name() -> String {
    if let Ok(p) = std::env::var(nodespace_proto::socket::SOCKET_ENV_VAR) {
        return p;
    }
    nodespace_proto::socket::DAEMON_PIPE_NAME.to_string()
}

/// Owns a self-relative `SECURITY_DESCRIPTOR` allocated by
/// `ConvertStringSecurityDescriptorToSecurityDescriptorW`, plus the
/// `SECURITY_ATTRIBUTES` struct pointing at it. The descriptor is freed
/// (`LocalFree`) on drop, per that function's documented contract; the
/// pointer returned by [`Self::as_mut_ptr`] is only valid while `self` is
/// alive.
#[cfg(windows)]
struct OwnerOnlySecurityAttributes {
    attrs: windows_sys::Win32::Security::SECURITY_ATTRIBUTES,
    security_descriptor: *mut std::ffi::c_void,
}

#[cfg(windows)]
impl OwnerOnlySecurityAttributes {
    fn as_mut_ptr(&mut self) -> *mut std::ffi::c_void {
        &mut self.attrs as *mut windows_sys::Win32::Security::SECURITY_ATTRIBUTES
            as *mut std::ffi::c_void
    }
}

#[cfg(windows)]
impl Drop for OwnerOnlySecurityAttributes {
    fn drop(&mut self) {
        if !self.security_descriptor.is_null() {
            // Safety: `security_descriptor` was allocated by
            // `ConvertStringSecurityDescriptorToSecurityDescriptorW`, which is
            // documented to require freeing via `LocalFree`.
            unsafe {
                windows_sys::Win32::Foundation::LocalFree(
                    self.security_descriptor as windows_sys::Win32::Foundation::HLOCAL,
                );
            }
        }
    }
}

/// Reads a NUL-terminated wide string starting at `ptr`.
///
/// # Safety
/// `ptr` must point at a valid, NUL-terminated UTF-16 string that stays valid
/// for the duration of this call.
#[cfg(windows)]
unsafe fn pwstr_to_string(ptr: *const u16) -> String {
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
}

/// The current process token's user SID, formatted as an SDDL SID string
/// (`S-1-5-21-...`) for embedding directly into an SDDL security descriptor
/// string.
#[cfg(windows)]
fn current_user_sid_string() -> std::io::Result<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let result = (|| {
            let mut needed: u32 = 0;
            // First call is expected to fail with ERROR_INSUFFICIENT_BUFFER
            // and report the buffer size actually needed in `needed`.
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
            if needed == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut buf: Vec<u8> = vec![0u8; needed as usize];
            if GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr() as *mut std::ffi::c_void,
                needed,
                &mut needed,
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            // Safety: `buf` was just filled by `GetTokenInformation` for
            // `TokenUser`, which documents its output as a `TOKEN_USER`.
            let token_user = &*(buf.as_ptr() as *const TOKEN_USER);

            let mut sid_str_ptr: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(token_user.User.Sid, &mut sid_str_ptr) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let sid_string = pwstr_to_string(sid_str_ptr);
            LocalFree(sid_str_ptr as HLOCAL);
            Ok(sid_string)
        })();
        CloseHandle(token);
        result
    }
}

/// Builds a `SECURITY_ATTRIBUTES`/DACL restricting access to exactly the
/// current user -- the Windows equivalent of the Unix `0o600` guarantee
/// [`bind_uds_owner_only`] provides.
///
/// Built via SDDL rather than hand-rolled ACL/ACE byte buffers:
/// `O:{sid}D:P(A;;GA;;;{sid})` sets the security descriptor's owner to the
/// current user's real SID and grants that same SID -- and only that SID --
/// `GENERIC_ALL` on the pipe; `P` marks the DACL protected so nothing can
/// inherit extra access onto it later. No ACE is present for any other
/// principal, so `Everyone`/`Authenticated Users`/every other local
/// principal is implicitly denied: the same "no ACE, no access" default
/// Windows applies to any securable object.
///
/// Verified on real Windows hardware against a live pipe handle: the
/// security descriptor read back via `GetSecurityInfo` has the current
/// user as owner and exactly one DACL ACE (granting that same user
/// `FILE_ALL_ACCESS` -- the object-type-specific form Windows resolves
/// `GENERIC_ALL` to for a file/pipe object), with no ACE for any other
/// principal.
#[cfg(windows)]
fn owner_only_security_attributes() -> std::io::Result<OwnerOnlySecurityAttributes> {
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    let sid = current_user_sid_string()?;
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;;GA;;;{sid})\0")
        .encode_utf16()
        .collect();

    let mut security_descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // Safety: `sddl` is a NUL-terminated wide string alive for this call;
    // `security_descriptor` is an out-param this function initializes.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut security_descriptor,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(OwnerOnlySecurityAttributes {
        attrs: SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security_descriptor,
            bInheritHandle: 0,
        },
        security_descriptor,
    })
}

/// Creates one Named Pipe server instance restricted to the current user
/// (ADR-052 -- the Windows equivalent of [`bind_uds_owner_only`]'s `0o600`).
///
/// `first_instance` must be `true` exactly once per pipe name: the call that
/// claims it. `first_pipe_instance(true)` sets `FILE_FLAG_FIRST_PIPE_INSTANCE`,
/// which makes pipe creation fail loudly with `ERROR_ACCESS_DENIED` if any
/// instance of the name already exists -- including one pre-created by a
/// malicious local process squatting the name before this daemon starts
/// (verified on real Windows hardware: pre-creating the name and then
/// retrying with `first_instance: true` fails with exactly that error).
/// Every later instance, created in the accept loop to serve the next client
/// after the current one disconnects, must pass `false`: by then the name is
/// legitimately owned by this daemon, and `FIRST_PIPE_INSTANCE` would
/// spuriously fail against the daemon's own earlier instance rather than an
/// intruder's.
#[cfg(windows)]
fn create_owner_only_pipe(
    name: &str,
    first_instance: bool,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let mut sa = owner_only_security_attributes()?;
    // Safety: `sa.as_mut_ptr()` points at a fully-initialized
    // `SECURITY_ATTRIBUTES` whose `lpSecurityDescriptor` is a valid
    // self-relative security descriptor; `sa` is not dropped (so the
    // descriptor stays valid) until after this call returns -- Windows
    // duplicates the descriptor into the kernel object during the call and
    // does not need it to outlive it.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first_instance)
            .create_with_security_attributes_raw(name, sa.as_mut_ptr())
    }
}

/// `tao`'s event loop must own the main thread on macOS (NSApplication is
/// main-thread-only). So `main` builds the tokio runtime explicitly, hands
/// it to a worker thread that hosts the gRPC server, and lets `tray::run`
/// take over the main thread.
///
/// Headless mode is supported for systems that don't have a display (Linux
/// CI, headless servers): if `NODESPACED_HEADLESS=1` is set, the tray loop
/// is skipped and we fall back to a pure async `main` that exits on signals.
///
/// Live validation found the tray-mode shutdown sequence can intermittently
/// stall indefinitely after GPU/model teardown completes and before this
/// function's own "shutdown complete" log line -- observed durations ranged
/// from ~24s to several minutes, non-deterministic, root cause not fully
/// pinned down (a background task, e.g. from `SharedServices`, not finishing
/// promptly is the leading suspect, but this has not been conclusively
/// isolated). [`SHUTDOWN_WATCHDOG_TIMEOUT`] bounds how long a deliberate quit
/// (tray "Quit", SIGTERM/SIGINT, or the app's own quit path) waits for the
/// ENTIRE post-`tray::run` sequence -- gRPC drain, `shutdown_all()` across
/// every open database, and GPU/model teardown together, not just the
/// specific tail where the hang has been observed so far -- before forcing
/// an exit. Bounding the whole sequence rather than just the suspected tail
/// is deliberate: today `shutdown_all()`/gRPC drain are cheap and fast, but
/// nothing architecturally guarantees that stays true, and a hang anywhere
/// in this path has the identical symptom (Quit does nothing at all,
/// forever), which is worse than a slightly abrupt forced exit. Exits `0`
/// (not an error code) even when forced, since this always fires only after
/// the user (or the OS) already asked to quit -- treating it as a "failure"
/// would make launchd's now-conditional `KeepAlive` (see `write_plist` in
/// `daemon_setup.rs`) restart the daemon right back up, undoing the very
/// quit this exists to guarantee.
const SHUTDOWN_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(15);

fn main() -> Result<()> {
    // Early-exit flags — handled before tracing/runtime init so the installer
    // postinstall script can query these without spinning up the full daemon.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--edition") {
        println!("{}", edition());
        return Ok(());
    }
    if args.iter().any(|a| a == "--version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Initialise OTLP tracing when NODESPACE_MLFLOW_URL is set (dev only).
    // Keep the provider alive for the duration of main so the background
    // exporter thread is not torn down prematurely.
    let _otlp_provider = otlp_tracer::init_tracer();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;

    if headless() {
        return runtime.block_on(async { serve_headless().await });
    }

    // The tray's seed closure runs synchronously when `tray::run` is called,
    // launching the gRPC server on the tokio runtime so the daemon is
    // serving as soon as the tray appears. The returned `JoinHandle` flows
    // back out of `tray::run` once the tray loop exits -- either the user
    // picked Quit, or `bridge_grpc_completion_to_tray` told it the gRPC task
    // stopped on its own (a signal drained it, or it failed).
    let runtime_handle = runtime.handle().clone();
    let grpc_handle = tray::run(move |controller| {
        let bridge_controller = controller.clone();
        let task = runtime_handle.spawn(async move { serve_grpc(controller).await });
        runtime_handle.spawn(bridge_grpc_completion_to_tray(task, bridge_controller))
    })?;

    // `tray::run` returned, so the tray loop has exited (Quit, a signal, or
    // a gRPC task failure). Wait for the gRPC server to finish draining
    // before we drop the runtime — otherwise in-flight RPCs would be killed
    // mid-response. Bounded by a watchdog
    // (see SHUTDOWN_WATCHDOG_TIMEOUT's doc comment): if graceful shutdown
    // doesn't finish in time, force-exit rather than let a stuck teardown
    // make Quit hang forever.
    let defused = arm_shutdown_watchdog(SHUTDOWN_WATCHDOG_TIMEOUT, || {
        tracing::error!(
            timeout_secs = SHUTDOWN_WATCHDOG_TIMEOUT.as_secs(),
            "Graceful shutdown did not complete in time -- forcing exit. \
             If you see this, please report it: something in daemon \
             teardown is hanging."
        );
        std::process::exit(0);
    });

    runtime
        .block_on(grpc_handle)
        .context("gRPC task panicked")?
        .context("gRPC server returned an error")?;
    defused.store(true, Ordering::SeqCst);

    tracing::info!("nodespaced shutdown complete");
    Ok(())
}

/// Awaits `task` -- the gRPC server's own `JoinHandle` -- and tells the tray
/// loop once it resolves, then re-surfaces the exact same outcome so the
/// existing `.context("gRPC task panicked")?.context("gRPC server returned
/// an error")?` handling in `main` downstream of `grpc_handle` is
/// unaffected: a panic inside `task` still resolves *this* handle as a panic
/// too (via `resume_unwind`, not swallowed into a returned `Err`), and a
/// clean or errored return passes through unchanged.
///
/// This is the bridge for two ways the gRPC task can stop that the tray
/// loop previously had no way to learn about at all: an OS signal
/// (SIGTERM/SIGINT) draining it via `combined_shutdown`, or it panicking or
/// returning an error internally. Both used to leave `task` finished with
/// nothing watching it, so the tao loop -- which only reacts to the tray's
/// own "Quit" menu click -- sat forever with a live tray icon fronting a
/// dead gRPC server. A user-initiated Quit still reaches `ControlFlow::Exit`
/// on its own through the menu handler and never depends on this path.
///
/// One asymmetry with the Quit path worth knowing: `task` doesn't resolve
/// until `serve_grpc`'s own post-shutdown drain (`shutdown_all` + shared GPU
/// release) has already finished, so this bridge only fires once that drain
/// is done -- whereas the Quit path reaches `tray::run`'s return, and so
/// arms `main`'s shutdown watchdog, *before* that same drain runs. If that
/// drain is ever the thing hanging, a SIGTERM never reaches this bridge
/// either, and the watchdog never arms to force it through. Fixing that
/// class of hang is a separate, still-open problem (see
/// `SHUTDOWN_WATCHDOG_TIMEOUT`'s doc comment) -- this bridge only fixes the
/// case where the gRPC task actually does finish and nothing was listening.
async fn bridge_grpc_completion_to_tray(
    task: tokio::task::JoinHandle<Result<()>>,
    controller: tray::TrayController,
) -> Result<()> {
    let outcome = task.await;
    controller.grpc_task_finished();
    resurface_grpc_task_outcome(outcome)
}

/// Pure mapping from the gRPC task's raw `JoinHandle` outcome back to the
/// `Result<()>` `main`'s existing `.context("gRPC task panicked")?
/// .context("gRPC server returned an error")?` chain expects -- split out of
/// [`bridge_grpc_completion_to_tray`] so this half is unit-testable without a
/// real tao event loop, which `TrayController` needs a live `EventLoopProxy`
/// (and therefore an actual platform event loop) to construct.
fn resurface_grpc_task_outcome(outcome: Result<Result<()>, tokio::task::JoinError>) -> Result<()> {
    match outcome {
        Ok(result) => result,
        Err(join_err) if join_err.is_panic() => std::panic::resume_unwind(join_err.into_panic()),
        Err(join_err) => Err(join_err).context("gRPC task did not complete"),
    }
}

#[cfg(test)]
mod grpc_completion_bridge_tests {
    use super::resurface_grpc_task_outcome;

    /// The signal-drained/clean-shutdown case (Gap 1): `serve_grpc` returning
    /// `Ok(())` after a SIGTERM must still surface as `Ok(())` here, so
    /// `main` proceeds to a clean, zero-code exit rather than treating a
    /// normal shutdown as a failure.
    #[tokio::test]
    async fn a_successful_task_result_passes_through_unchanged() {
        let handle = tokio::spawn(async { Ok(()) });
        let outcome = handle.await;

        assert!(resurface_grpc_task_outcome(outcome).is_ok());
    }

    /// An internal error returned by `serve_grpc` (Gap 2, non-panic case)
    /// must still surface as `Err` here, so `main`'s `?` propagates it and
    /// the process exits nonzero -- which is what makes launchd's
    /// conditional `KeepAlive` restart the daemon.
    #[tokio::test]
    async fn an_errored_task_result_passes_through_as_an_error() {
        let handle = tokio::spawn(async { Err::<(), _>(anyhow::anyhow!("boom")) });
        let outcome = handle.await;

        let err = resurface_grpc_task_outcome(outcome).expect_err("must stay an error");
        assert!(
            err.to_string().contains("boom"),
            "the original error's message must survive, got: {err}"
        );
    }

    /// The other half of Gap 2: a genuine panic inside the gRPC task must
    /// still resolve *this* function's own caller as a panic (via
    /// `resume_unwind`), not be silently downgraded to a returned `Err`.
    /// `main`'s two-step `.context("gRPC task panicked")?` specifically
    /// depends on that distinction to report the right failure mode.
    #[tokio::test]
    #[should_panic(expected = "boom")]
    async fn a_panicking_task_repanics_here_instead_of_becoming_an_error() {
        let handle = tokio::spawn(async { panic!("boom") });
        let outcome = handle.await;

        let _ = resurface_grpc_task_outcome(outcome);
    }
}

/// Spawns a background thread that runs `on_timeout` once, after `timeout`,
/// unless the returned flag is set to `true` first (by the caller, once the
/// thing being bounded actually finishes). The watchdog thread only reads
/// the flag and calls `on_timeout` — it needs no cooperation from whatever
/// might be stuck, which is the whole point: it still fires even if the
/// bounded work is wedged on a synchronous call that can't be cancelled.
///
/// Split out from `main`'s tray-mode shutdown path so the arm/defuse timing
/// logic itself is unit-testable without needing to trigger a real process
/// exit — production passes `std::process::exit`, tests pass something
/// observable instead.
fn arm_shutdown_watchdog(
    timeout: Duration,
    on_timeout: impl FnOnce() + Send + 'static,
) -> Arc<AtomicBool> {
    let defused = Arc::new(AtomicBool::new(false));
    let watcher = defused.clone();
    std::thread::spawn(move || {
        std::thread::sleep(timeout);
        if !watcher.load(Ordering::SeqCst) {
            on_timeout();
        }
    });
    defused
}

#[cfg(test)]
mod shutdown_watchdog_tests {
    use super::arm_shutdown_watchdog;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::time::Duration;

    /// The bug this exists to prevent: if graceful shutdown never
    /// completes, `on_timeout` must still fire so the process doesn't hang
    /// forever.
    ///
    /// Uses a channel rather than a fixed sleep-then-check margin: this test
    /// waits exactly as long as it takes for `on_timeout` to actually fire
    /// (fast in practice), bounded by a generous 2s `recv_timeout` so a
    /// loaded CI runner delaying the watchdog thread's own scheduling can't
    /// produce a false failure the way a fixed short sleep could.
    #[test]
    fn fires_on_timeout_when_never_defused() {
        let (tx, rx) = mpsc::channel();
        let _defused = arm_shutdown_watchdog(Duration::from_millis(20), move || {
            let _ = tx.send(());
        });

        rx.recv_timeout(Duration::from_secs(2))
            .expect("on_timeout must fire once the timeout elapses with no defuse");
    }

    /// The normal, fast-shutdown path: `on_timeout` must NOT fire once the
    /// caller marks the watched work as actually finished. Proving a
    /// negative still requires waiting out the deadline (a channel alone
    /// can't shortcut that), but the wait here is bounded to the timeout
    /// plus a fixed, generous margin rather than a wholly separate guessed
    /// sleep duration.
    #[test]
    fn does_not_fire_once_defused_before_timeout() {
        let (tx, rx) = mpsc::channel();
        let defused = arm_shutdown_watchdog(Duration::from_millis(50), move || {
            let _ = tx.send(());
        });

        defused.store(true, Ordering::SeqCst);

        match rx.recv_timeout(Duration::from_millis(300)) {
            // Timeout: on_timeout was never called at all, so `tx` is still
            // parked inside it, unsent. Disconnected: on_timeout's closure
            // (and the `tx` it owns) was dropped without sending, once the
            // watchdog thread found `defused` set and skipped calling it.
            // Both correctly mean "never fired" -- only Ok(()) means it did.
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {}
            Ok(()) => {
                panic!("on_timeout must not fire once the caller defused it before the deadline")
            }
        }
    }
}

/// Wraps a shutdown-trigger future (e.g. `combined_shutdown`) so that the
/// instant it resolves, a watchdog is armed for whatever tonic does next --
/// its own connection/stream draining inside `serve_with_incoming_shutdown`.
///
/// Live investigation localized the actual hang to exactly this
/// window: `shutdown_all`/`release_shared_gpu` (bounded by
/// [`drain_and_release_gpu`]'s own watchdog) are always fast, microseconds
/// to milliseconds, even during a stalled run -- the delay sits entirely
/// between the shutdown signal firing and `serve_with_incoming_shutdown`'s
/// future resolving. A client with an in-flight streaming RPC open at the
/// moment of shutdown (the `WatchNodes` live-update subscription is the
/// leading suspect) can make tonic's graceful drain wait; killing the GUI
/// client before signaling the daemon made the stall disappear entirely
/// across repeated trials, while leaving it connected reproduced it
/// intermittently. Neither existing watchdog covers this: `main`'s outer
/// one never arms at all on the signal path (see
/// [`bridge_grpc_completion_to_tray`]'s doc comment), and
/// `drain_and_release_gpu`'s only starts once its own timer is taken, which
/// is *after* this gap.
///
/// Returns a slot the caller must pass to
/// [`defuse_serve_drain_watchdog`] once `serve_with_incoming_shutdown`
/// itself has returned. Tonic hands nothing back through the shutdown
/// future's own output (it stays `()`), so this side channel is how the
/// armed watchdog's handle escapes the future's scope and reaches the code
/// after the `.await`.
///
/// `timeout`/`on_timeout` are taken as parameters rather than hardcoded --
/// same reason [`arm_shutdown_watchdog`] does -- so the arm-only-after-
/// resolution timing is unit-testable with a short timeout and an
/// observable side effect instead of a real 15-second wait and a real
/// process exit. Production passes [`SHUTDOWN_WATCHDOG_TIMEOUT`] and
/// `std::process::exit`.
fn watch_for_shutdown_signal(
    shutdown_future: impl std::future::Future<Output = ()> + Send + 'static,
    timeout: Duration,
    on_timeout: impl FnOnce() + Send + 'static,
) -> (
    impl std::future::Future<Output = ()>,
    Arc<std::sync::Mutex<Option<Arc<AtomicBool>>>>,
) {
    let slot: Arc<std::sync::Mutex<Option<Arc<AtomicBool>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let slot_for_future = slot.clone();
    let wrapped = async move {
        shutdown_future.await;
        let defused = arm_shutdown_watchdog(timeout, on_timeout);
        *slot_for_future.lock().unwrap() = Some(defused);
    };
    (wrapped, slot)
}

/// Defuses the watchdog [`watch_for_shutdown_signal`] armed, once
/// `serve_with_incoming_shutdown` has actually returned. A no-op if the
/// shutdown future never got the chance to arm one -- shouldn't happen in
/// practice, since `serve_with_incoming_shutdown` cannot return before its
/// own shutdown future resolves, but a missing watchdog is a strictly safer
/// failure mode here than panicking on it.
fn defuse_serve_drain_watchdog(slot: &std::sync::Mutex<Option<Arc<AtomicBool>>>) {
    if let Some(defused) = slot.lock().unwrap().take() {
        defused.store(true, Ordering::SeqCst);
    }
}

/// Drain every open database's compute, then release the shared GPU context
/// once. Common to both platforms' tray-mode `serve_grpc`.
///
/// Bounded by its own watchdog for the same reason
/// [`watch_for_shutdown_signal`] bounds tonic's connection drain: `main`'s
/// outer watchdog only arms once `tray::run()` has returned, which on a
/// signal-triggered shutdown doesn't happen until `serve_grpc`'s whole body
/// -- this drain included -- has already resolved (see
/// [`bridge_grpc_completion_to_tray`]'s doc comment). Live investigation
/// found this specific drain is not actually where the hang lives (see
/// `watch_for_shutdown_signal`'s doc comment for where it is),
/// but the watchdog stays here regardless as real, correct protection for
/// this segment too -- both are true simultaneously: this drain has always
/// measured fast, and an unprotected window here would still be a bug
/// waiting to happen the day that stops being true.
async fn drain_and_release_gpu(
    shutdown_manager: Arc<DatabaseManager>,
    shared_model: watch::Receiver<Option<Arc<EmbeddingService>>>,
) {
    let defused = arm_shutdown_watchdog(SHUTDOWN_WATCHDOG_TIMEOUT, || {
        tracing::error!(
            timeout_secs = SHUTDOWN_WATCHDOG_TIMEOUT.as_secs(),
            "shutdown_all/release_shared_gpu did not complete in time -- forcing exit. If you \
             see this, please report it: something in daemon teardown is hanging."
        );
        std::process::exit(0);
    });

    let shutdown_started = std::time::Instant::now();
    shutdown_manager.shutdown_all().await;
    tracing::info!(elapsed = ?shutdown_started.elapsed(), "shutdown_all finished");
    let gpu_release_started = std::time::Instant::now();
    release_shared_gpu(&shared_model).await;
    tracing::info!(elapsed = ?gpu_release_started.elapsed(), "release_shared_gpu finished");

    defused.store(true, Ordering::SeqCst);
}

#[cfg(test)]
mod drain_and_release_gpu_tests {
    /// Balanced-brace extraction of the source text starting at `decl_start`
    /// (the byte offset of a `fn`/fn-attribute line), from its own opening
    /// `{` through the matching closing `}`. Boundary-agnostic to whatever
    /// text follows in the file -- a name-based end marker (e.g. `.find("fn
    /// next_thing")`) silently grows to include anything inserted between
    /// the target and that marker, which is exactly what made a similar test
    /// elsewhere in this codebase tautological before it was fixed the same
    /// way this one is written.
    fn braced_body(source: &str, decl_start: usize) -> &str {
        let body_start = source[decl_start..]
            .find('{')
            .map(|i| decl_start + i)
            .expect("no opening brace found after decl_start");
        let mut depth = 0i32;
        let mut end = body_start;
        for (i, ch) in source[body_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = body_start + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        &source[decl_start..end]
    }

    fn function_source(name: &str) -> &'static str {
        let source = include_str!("main.rs");
        let sig = format!("fn {name}(");
        let start = source
            .find(&sig)
            .unwrap_or_else(|| panic!("{name} not found in main.rs"));
        braced_body(source, start)
    }

    /// The exact gap this function exists to close: a stall in
    /// `shutdown_all`/`release_shared_gpu` must be bounded regardless of
    /// which path triggered shutdown -- not just the tray-Quit path that
    /// `main`'s own outer watchdog happens to cover for unrelated reasons
    /// (see this function's own doc comment).
    #[test]
    fn arms_a_watchdog_before_the_drain_and_defuses_it_after() {
        let src = function_source("drain_and_release_gpu");
        let arm_pos = src
            .find("arm_shutdown_watchdog")
            .expect("must arm a watchdog");
        let shutdown_all_pos = src
            .find("shutdown_manager.shutdown_all()")
            .expect("must call shutdown_all");
        let release_pos = src
            .find("release_shared_gpu(")
            .expect("must call release_shared_gpu");
        let defuse_pos = src
            .find("defused.store(true")
            .expect("must defuse the watchdog once the drain finishes");

        assert!(
            arm_pos < shutdown_all_pos,
            "the watchdog must be armed BEFORE shutdown_all runs, not after -- arming it after \
             would leave a stall inside shutdown_all itself unprotected"
        );
        assert!(
            shutdown_all_pos < release_pos,
            "shutdown_all must run before release_shared_gpu (draining database compute before \
             releasing the GPU context it may still be using)"
        );
        assert!(
            release_pos < defuse_pos,
            "the watchdog must be defused only AFTER both steps finish -- defusing earlier would \
             leave a genuine stall in release_shared_gpu unprotected"
        );
    }

    /// Both tray-mode `serve_grpc` implementations AND both headless
    /// `serve_headless` implementations (Unix and Windows, four functions
    /// total) must route their post-serve drain through
    /// `drain_and_release_gpu`, not call `shutdown_all`/`release_shared_gpu`
    /// directly -- a function that regresses back to the old inline pattern
    /// silently loses watchdog coverage for exactly the segment already
    /// confirmed to hang.
    ///
    /// `serve_headless` used to be exactly that regression, permanently: it
    /// never routed through either helper at all, in either implementation
    /// -- unlike tray mode, headless shutdown had no timeout anywhere in its
    /// path. A hang there had no forced exit to fall back on, which is
    /// exactly the shape core#2758 hit (a `nodespaced` that had to be
    /// `SIGKILL`ed after `SIGTERM`/`SIGINT` never returned).
    #[test]
    fn every_serve_loop_uses_the_shared_watchdog_helpers() {
        let source = include_str!("main.rs");
        for (label, marker) in [
            ("unix serve_grpc", "#[cfg(unix)]\nasync fn serve_grpc"),
            ("windows serve_grpc", "#[cfg(windows)]\nasync fn serve_grpc"),
            (
                "unix serve_headless",
                "#[cfg(unix)]\nasync fn serve_headless",
            ),
            (
                "windows serve_headless",
                "#[cfg(windows)]\nasync fn serve_headless",
            ),
        ] {
            let start = source
                .find(marker)
                .unwrap_or_else(|| panic!("{label} not found"));
            let body = braced_body(source, start);
            assert!(
                body.contains("drain_and_release_gpu("),
                "{label} must route its post-serve drain through drain_and_release_gpu, \
                 not call shutdown_all/release_shared_gpu inline"
            );
            assert!(
                body.contains("watch_for_shutdown_signal(")
                    && body.contains("defuse_serve_drain_watchdog("),
                "{label} must wrap its shutdown-trigger future with watch_for_shutdown_signal \
                 and defuse it with defuse_serve_drain_watchdog after \
                 serve_with_incoming_shutdown returns -- a function missing either call silently \
                 loses watchdog coverage for tonic's own connection/stream drain"
            );
        }
    }
}

#[cfg(test)]
mod watch_for_shutdown_signal_tests {
    use super::{defuse_serve_drain_watchdog, watch_for_shutdown_signal};
    use std::sync::mpsc;
    use std::time::Duration;

    /// The watchdog must not fire while the shutdown-trigger future hasn't
    /// resolved yet -- it has to arm only once shutdown is actually
    /// signaled, not for the server's whole normal-operation lifetime
    /// (which would fire on every healthy run, not just a stalled
    /// shutdown). `std::future::pending` never resolves, so if
    /// `on_timeout` ever fires here, arming happened before resolution.
    /// Multi-threaded runtime: the test thread blocks on the synchronous
    /// `recv_timeout` below, so the spawned task needs a real worker thread
    /// of its own to be polled at all -- on the default single-threaded
    /// runtime it would never run, making this pass vacuously regardless
    /// of whether the code under test is correct.
    #[tokio::test(flavor = "multi_thread")]
    async fn does_not_arm_before_the_shutdown_future_resolves() {
        let (tx, rx) = mpsc::channel();
        let (wrapped, _slot) = watch_for_shutdown_signal(
            std::future::pending::<()>(),
            Duration::from_millis(30),
            move || {
                let _ = tx.send(());
            },
        );
        tokio::spawn(wrapped);

        match rx.recv_timeout(Duration::from_millis(200)) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            other => panic!(
                "the watchdog must not fire before the shutdown future resolves (it never does \
                 here), got {other:?}"
            ),
        }
    }

    /// Once the shutdown-trigger future resolves, the watchdog is armed for
    /// real and fires on schedule if nothing defuses it -- this is what
    /// bounds a stall inside `serve_with_incoming_shutdown` itself.
    /// Multi-threaded runtime -- same reason as the test above: the spawned
    /// task needs a real worker thread to be polled while the test thread
    /// blocks on `recv_timeout`.
    #[tokio::test(flavor = "multi_thread")]
    async fn arms_and_fires_once_the_shutdown_future_resolves_if_never_defused() {
        let (tx, rx) = mpsc::channel();
        let (wrapped, _slot) =
            watch_for_shutdown_signal(async {}, Duration::from_millis(30), move || {
                let _ = tx.send(());
            });
        tokio::spawn(wrapped);

        match rx.recv_timeout(Duration::from_millis(300)) {
            Ok(()) => {}
            other => panic!("the watchdog must fire once armed and never defused, got {other:?}"),
        }
    }

    /// `defuse_serve_drain_watchdog`, called once `serve_with_incoming_shutdown`
    /// has actually returned, must prevent the watchdog from firing --
    /// otherwise a perfectly healthy drain would still force-exit the
    /// process every time.
    #[tokio::test]
    async fn defusing_after_resolution_prevents_the_watchdog_from_firing() {
        let (tx, rx) = mpsc::channel();
        let (wrapped, slot) =
            watch_for_shutdown_signal(async {}, Duration::from_millis(30), move || {
                let _ = tx.send(());
            });
        wrapped.await; // resolves immediately, arming the watchdog
        defuse_serve_drain_watchdog(&slot);

        match rx.recv_timeout(Duration::from_millis(200)) {
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {}
            Ok(()) => panic!("defuse_serve_drain_watchdog must prevent the watchdog from firing"),
        }
    }
}

/// Whether `main` should take the plain async [`serve_headless`] path instead
/// of handing the main thread to `tray::run`'s `tao`/`NSApplication` event
/// loop.
///
/// Defaults to `false` (tray mode) so the desktop app's bundled daemon --
/// which never sets this variable -- keeps its tray icon. That default is
/// exactly what made the `nodespace-cli` Homebrew formula's daemon (no GUI,
/// no bundled app, nothing that wants a tray icon) hang forever on
/// `SIGTERM`/`SIGINT` with **zero clients ever attached**: unless
/// `NODESPACED_HEADLESS=1` is set, that "headless" CLI daemon was *actually*
/// entering tray mode too, since its `brew services` launch never set the
/// variable either.
///
/// Root-caused with a live thread sample (`sample`, macOS) on a hung,
/// zero-client instance taken well after `kill -TERM`: every `tokio-rt-
/// worker` thread and the I/O driver were fully parked (`kevent`/condvar
/// wait, 0% CPU, no busy loop), and the daemon's own `"SIGTERM received"`
/// log line -- which fires synchronously the instant
/// [`install_shutdown_handler`]'s future resolves, before any other work --
/// never printed at all, even minutes later. The identical binary, signaled
/// the identical way, with `NODESPACED_HEADLESS=1` set (so the tao/
/// `NSApplication` event loop never starts and the main thread runs
/// [`serve_headless`] directly instead) logs `"SIGTERM received"` and exits
/// cleanly in well under a second, every time, including after real gRPC
/// traffic. So once `tao`'s event loop has taken the main thread on macOS,
/// something about that state stops tokio's own `SIGTERM`/`SIGINT` handlers
/// (installed via `tokio::signal::unix::signal`, independent low-level
/// `sigaction` registration) from ever being invoked -- this function's
/// `false` default is what routes a should-be-headless deployment into that
/// state. The exact AppKit/tao mechanism was not pinned down further (that
/// would need platform-level tracing of `sigaction`, out of scope here);
/// what's fixed here is making sure a real headless deployment never
/// exercises that path in the first place, via the Homebrew formula setting
/// this variable explicitly (`scripts/update-homebrew-formula.ts`) rather
/// than relying on this default. Tray mode's *own* SIGTERM handling
/// remaining fragile when nothing pumps its run loop is a separate,
/// still-open problem -- the same class core#2357's own doc comments
/// already flag as unresolved (see [`bridge_grpc_completion_to_tray`]).
fn headless() -> bool {
    matches!(std::env::var("NODESPACED_HEADLESS").as_deref(), Ok("1"))
}

/// Returns the build edition: "pro" when compiled with `--features pro`, otherwise "community".
fn edition() -> &'static str {
    if is_pro_build() {
        "pro"
    } else {
        "community"
    }
}

/// Headless server loop. Used by Linux CI and any environment without a
/// display server. Shutdown is signal-driven (SIGTERM / SIGINT), there is
/// no tray.
///
/// Wrapped by the same two watchdogs [`serve_grpc`] uses (see
/// [`watch_for_shutdown_signal`] and [`drain_and_release_gpu`]) — this path
/// used to call `serve_with_incoming_shutdown`/`shutdown_all`/
/// `release_shared_gpu` directly, with no timeout anywhere, so a stall
/// anywhere in shutdown had no bound at all and no forced exit to fall back
/// on. That gap was real, not theoretical: it is exactly how a `nodespaced`
/// with no client ever attached turned out to require `SIGKILL` after
/// `SIGTERM`/`SIGINT` — see [`headless`]'s own doc comment for how that hang
/// was actually root-caused (a *different* bug, in `main`'s tray-mode
/// default, not in the drain logic here). This wiring stays regardless,
/// as real protection for this path against a future stall in either
/// segment, matching [`drain_and_release_gpu`]'s own reasoning for keeping
/// its watchdog even once its originally-suspected segment was cleared.
#[cfg(unix)]
async fn serve_headless() -> Result<()> {
    use tokio_stream::wrappers::UnixListenerStream;

    let sock = socket_path();
    let db_path = resolve_db_path()?;

    tracing::info!(requested_db_path = %db_path.display(), sock = %sock.display(), "Starting nodespaced (headless)");

    let shutdown = install_shutdown_handler().context("Failed to install signal handlers")?;
    // _model_task: dropping a JoinHandle does not cancel the task in tokio — it detaches.
    let (shared, _model_task) = build_shared_services().await?;
    let (manager, bundle) = open_default_database(&db_path, shared.context.clone()).await?;
    // Reap idle non-default databases so a switched-away database stops consuming
    // compute (ADR-053: per-database compute scoping).
    manager.spawn_idle_reaper();
    let shared_model = shared.context.model.clone();
    let shutdown_manager = manager.clone();

    if let Some(parent) = sock.parent() {
        // Owner-only from birth (and re-restricted if it already existed at a
        // wider mode) — this directory is what `bind_uds_owner_only` checks
        // fail-closed before binding, and what closes the bind-to-chmod window
        // on the socket itself (ADR-052).
        create_dir_owner_only(parent)
            .await
            .with_context(|| format!("Failed to create socket directory: {}", parent.display()))?;
    }
    let _ = tokio::fs::remove_file(&sock).await;
    let listener = bind_uds_owner_only(&sock)?;

    let (wrapped_shutdown, serve_drain_watchdog) =
        watch_for_shutdown_signal(shutdown, SHUTDOWN_WATCHDOG_TIMEOUT, || {
            tracing::error!(
                timeout_secs = SHUTDOWN_WATCHDOG_TIMEOUT.as_secs(),
                "serve_with_incoming_shutdown did not finish draining connections/streams in \
                 time -- forcing exit. If you see this, please report it: something is \
                 stalling tonic's own graceful shutdown."
            );
            std::process::exit(0);
        });

    tracing::info!(sock = %sock.display(), "gRPC server listening");

    let sock_cleanup = sock.clone();
    let base_services = BaseServices {
        node_service: bundle.node_service_grpc.clone(),
        agent_session: bundle.agent_session.clone(),
        import: bundle.import.clone(),
        settings: shared.settings,
        local_agent: bundle.local_agent.clone(),
        embeddings: bundle.embeddings_service_grpc.clone(),
        database: DatabaseServiceImpl::new(manager.clone()),
    };
    build_base_router(
        Server::builder().layer(DbManagerLayer::new(manager)),
        base_services,
    )
    .serve_with_incoming_shutdown(UnixListenerStream::new(listener), wrapped_shutdown)
    .await
    .context("gRPC server terminated with error")?;
    defuse_serve_drain_watchdog(&serve_drain_watchdog);
    let _ = tokio::fs::remove_file(&sock_cleanup).await;
    // Drain every open database's compute, then release the shared GPU once.
    drain_and_release_gpu(shutdown_manager, shared_model).await;
    Ok(())
}

/// Tray-driven server loop. Shutdown is owned by [`tray::TrayController`];
/// signal handlers still apply so packaged installs can `kill -TERM` the
/// daemon without going through the menu.
#[cfg(unix)]
async fn serve_grpc(controller: tray::TrayController) -> Result<()> {
    use tokio_stream::wrappers::UnixListenerStream;

    let sock = socket_path();
    let db_path = resolve_db_path()?;

    tracing::info!(requested_db_path = %db_path.display(), sock = %sock.display(), "Starting nodespaced (tray)");

    let signal_shutdown =
        install_shutdown_handler().context("Failed to install signal handlers")?;
    // _model_task: dropping a JoinHandle does not cancel the task in tokio — it detaches.
    let (shared, _model_task) = build_shared_services().await?;
    let (manager, bundle) = open_default_database(&db_path, shared.context.clone()).await?;
    // Reap idle non-default databases so a switched-away database stops consuming
    // compute (ADR-053: per-database compute scoping).
    manager.spawn_idle_reaper();
    // Fill the tray's Databases submenu and keep it in step. The registry is built
    // here, after the tray loop is already running, so the tray cannot be handed it
    // at startup — it is told instead. Safe if the tray hasn't finished
    // initializing: the snapshot is held and applied when it does.
    spawn_tray_database_sync(controller.clone(), manager.clone());
    let shared_model = shared.context.model.clone();
    let shutdown_manager = manager.clone();

    if let Some(parent) = sock.parent() {
        // Owner-only from birth (and re-restricted if it already existed at a
        // wider mode) — this directory is what `bind_uds_owner_only` checks
        // fail-closed before binding, and what closes the bind-to-chmod window
        // on the socket itself (ADR-052).
        create_dir_owner_only(parent)
            .await
            .with_context(|| format!("Failed to create socket directory: {}", parent.display()))?;
    }
    let _ = tokio::fs::remove_file(&sock).await;
    let listener = bind_uds_owner_only(&sock)?;

    let shutdown_controller = controller.clone();
    let (combined_shutdown, serve_drain_watchdog) = watch_for_shutdown_signal(
        async move {
            tokio::select! {
                _ = signal_shutdown => tracing::info!("OS signal triggered shutdown"),
                _ = shutdown_controller.shutdown() => tracing::info!("Tray Quit triggered shutdown"),
            }
        },
        SHUTDOWN_WATCHDOG_TIMEOUT,
        || {
            tracing::error!(
                timeout_secs = SHUTDOWN_WATCHDOG_TIMEOUT.as_secs(),
                "serve_with_incoming_shutdown did not finish draining connections/streams in \
                 time -- forcing exit. If you see this, please report it: a client (e.g. an \
                 open WatchNodes stream) is likely stalling tonic's own graceful shutdown."
            );
            std::process::exit(0);
        },
    );

    tracing::info!(sock = %sock.display(), "gRPC server listening");

    let sock_cleanup = sock.clone();
    let base_services = BaseServices {
        node_service: bundle.node_service_grpc.clone(),
        agent_session: bundle.agent_session.clone(),
        import: bundle.import.clone(),
        settings: shared.settings,
        local_agent: bundle.local_agent.clone(),
        embeddings: bundle.embeddings_service_grpc.clone(),
        database: DatabaseServiceImpl::new(manager.clone()),
    };
    build_base_router(
        Server::builder()
            .layer(TrayMetricsLayer::new(controller))
            .layer(DbManagerLayer::new(manager)),
        base_services,
    )
    .serve_with_incoming_shutdown(UnixListenerStream::new(listener), combined_shutdown)
    .await
    .context("gRPC server terminated with error")?;
    defuse_serve_drain_watchdog(&serve_drain_watchdog);
    let _ = tokio::fs::remove_file(&sock_cleanup).await;
    drain_and_release_gpu(shutdown_manager, shared_model).await;
    Ok(())
}

/// Newtype so we can implement tonic's `Connected` for `NamedPipeServer`.
/// `NamedPipeServer` is a transport; tonic's blanket bound requires `Connected`
/// on stream items so it can extract peer metadata for request extensions.
#[cfg(windows)]
struct NamedPipeConn(tokio::net::windows::named_pipe::NamedPipeServer);

#[cfg(windows)]
impl tonic::transport::server::Connected for NamedPipeConn {
    type ConnectInfo = ();
    fn connect_info(&self) -> Self::ConnectInfo {}
}

#[cfg(windows)]
impl tokio::io::AsyncRead for NamedPipeConn {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

#[cfg(windows)]
impl tokio::io::AsyncWrite for NamedPipeConn {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.0).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

/// Headless server loop for Windows — uses a Named Pipe instead of UDS.
///
/// Wrapped by the same two watchdogs the Unix headless loop and [`serve_grpc`]
/// use — see [`serve_headless`]'s own (Unix) doc comment for why this path
/// needs that coverage rather than calling the drain steps directly.
#[cfg(windows)]
async fn serve_headless() -> Result<()> {
    use tokio_util::sync::CancellationToken;

    let name = pipe_name();
    let db_path = resolve_db_path()?;

    tracing::info!(requested_db_path = %db_path.display(), pipe = %name, "Starting nodespaced (headless, Windows)");

    let shutdown = install_shutdown_handler().context("Failed to install signal handlers")?;
    // _model_task: dropping a JoinHandle does not cancel the task in tokio — it detaches.
    let (shared, _model_task) = build_shared_services().await?;
    let (manager, bundle) = open_default_database(&db_path, shared.context.clone()).await?;
    // Reap idle non-default databases so a switched-away database stops consuming
    // compute (ADR-053: per-database compute scoping).
    manager.spawn_idle_reaper();
    let shared_model = shared.context.model.clone();
    let shutdown_manager = manager.clone();

    // Claim the pipe name exclusively, with an owner-only DACL, before doing
    // anything else gRPC-related (ADR-052): fails loudly right here, at
    // startup, if another local process already owns or squatted this name,
    // instead of silently serving on a pipe reachable by every local
    // principal.
    let first_instance = create_owner_only_pipe(&name, true).with_context(|| {
        format!(
            "Failed to claim Named Pipe {name} (already in use, or squatted by another process?)"
        )
    })?;

    // CancellationToken is cloned into the acceptor stream so that
    // `server.connect().await` races against shutdown rather than blocking
    // indefinitely after tonic stops polling the stream.
    let cancel = CancellationToken::new();
    let cancel_stream = cancel.clone();
    let (wrapped_shutdown, serve_drain_watchdog) = watch_for_shutdown_signal(
        async move {
            shutdown.await;
            cancel.cancel();
        },
        SHUTDOWN_WATCHDOG_TIMEOUT,
        || {
            tracing::error!(
                timeout_secs = SHUTDOWN_WATCHDOG_TIMEOUT.as_secs(),
                "serve_with_incoming_shutdown did not finish draining connections/streams in \
                 time -- forcing exit. If you see this, please report it: something is \
                 stalling tonic's own graceful shutdown."
            );
            std::process::exit(0);
        },
    );

    tracing::info!(pipe = %name, "gRPC server listening (Named Pipe, owner-only DACL)");

    let incoming = {
        let name = name.clone();
        async_stream::stream! {
            // The first instance was already claimed above (with
            // `first_pipe_instance(true)`, so pipe-squatting fails loudly at
            // startup rather than here). Every later instance -- serving the
            // next client after the current one disconnects -- reuses the
            // name this daemon already owns, so it must pass `false`:
            // `first_pipe_instance(true)` only ever succeeds for the very
            // first instance of a name.
            let mut next = Some(first_instance);
            loop {
                let server = match next.take() {
                    Some(server) => server,
                    None => match create_owner_only_pipe(&name, false) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to create Named Pipe server instance");
                            yield Err(e);
                            return;
                        }
                    },
                };
                tokio::select! {
                    res = server.connect() => {
                        if let Err(e) = res { yield Err(e); return; }
                        yield Ok::<_, std::io::Error>(NamedPipeConn(server));
                    }
                    _ = cancel_stream.cancelled() => return,
                }
            }
        }
    };

    let base_services = BaseServices {
        node_service: bundle.node_service_grpc.clone(),
        agent_session: bundle.agent_session.clone(),
        import: bundle.import.clone(),
        settings: shared.settings,
        local_agent: bundle.local_agent.clone(),
        embeddings: bundle.embeddings_service_grpc.clone(),
        database: DatabaseServiceImpl::new(manager.clone()),
    };
    build_base_router(
        Server::builder().layer(DbManagerLayer::new(manager)),
        base_services,
    )
    .serve_with_incoming_shutdown(incoming, wrapped_shutdown)
    .await
    .context("gRPC server terminated with error")?;
    defuse_serve_drain_watchdog(&serve_drain_watchdog);
    // Drain every open database's compute, then release the shared GPU once.
    drain_and_release_gpu(shutdown_manager, shared_model).await;
    Ok(())
}

/// Tray-driven server loop for Windows — uses a Named Pipe instead of UDS.
#[cfg(windows)]
async fn serve_grpc(controller: tray::TrayController) -> Result<()> {
    use tokio_util::sync::CancellationToken;

    let name = pipe_name();
    let db_path = resolve_db_path()?;

    tracing::info!(requested_db_path = %db_path.display(), pipe = %name, "Starting nodespaced (tray, Windows)");

    let signal_shutdown =
        install_shutdown_handler().context("Failed to install signal handlers")?;
    // _model_task: dropping a JoinHandle does not cancel the task in tokio — it detaches.
    let (shared, _model_task) = build_shared_services().await?;
    let (manager, bundle) = open_default_database(&db_path, shared.context.clone()).await?;
    // Reap idle non-default databases so a switched-away database stops consuming
    // compute (ADR-053: per-database compute scoping).
    manager.spawn_idle_reaper();
    // Fill the tray's Databases submenu and keep it in step. The registry is built
    // here, after the tray loop is already running, so the tray cannot be handed it
    // at startup — it is told instead. Safe if the tray hasn't finished
    // initializing: the snapshot is held and applied when it does.
    spawn_tray_database_sync(controller.clone(), manager.clone());
    let shared_model = shared.context.model.clone();
    let shutdown_manager = manager.clone();

    let shutdown_controller = controller.clone();
    let (combined_shutdown, serve_drain_watchdog) = watch_for_shutdown_signal(
        async move {
            tokio::select! {
                _ = signal_shutdown => tracing::info!("OS signal triggered shutdown"),
                _ = shutdown_controller.shutdown() => tracing::info!("Tray Quit triggered shutdown"),
            }
        },
        SHUTDOWN_WATCHDOG_TIMEOUT,
        || {
            tracing::error!(
                timeout_secs = SHUTDOWN_WATCHDOG_TIMEOUT.as_secs(),
                "serve_with_incoming_shutdown did not finish draining connections/streams in \
                 time -- forcing exit. If you see this, please report it: a client (e.g. an \
                 open WatchNodes stream) is likely stalling tonic's own graceful shutdown."
            );
            std::process::exit(0);
        },
    );

    // Claim the pipe name exclusively, with an owner-only DACL, before doing
    // anything else gRPC-related (ADR-052): fails loudly right here, at
    // startup, if another local process already owns or squatted this name.
    // See the doc comment on `create_owner_only_pipe` for the full guarantee
    // and how it was verified.
    let first_instance = create_owner_only_pipe(&name, true).with_context(|| {
        format!(
            "Failed to claim Named Pipe {name} (already in use, or squatted by another process?)"
        )
    })?;

    tracing::info!(pipe = %name, "gRPC server listening (Named Pipe, owner-only DACL)");

    let cancel = CancellationToken::new();
    let cancel_stream = cancel.clone();
    let incoming = {
        let name = name.clone();
        async_stream::stream! {
            // See the equivalent comment in `serve_headless`: the first
            // instance is already claimed above; every later instance in
            // this loop must pass `false`.
            let mut next = Some(first_instance);
            loop {
                let server = match next.take() {
                    Some(server) => server,
                    None => match create_owner_only_pipe(&name, false) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to create Named Pipe server instance");
                            yield Err(e);
                            return;
                        }
                    },
                };
                tokio::select! {
                    res = server.connect() => {
                        if let Err(e) = res { yield Err(e); return; }
                        yield Ok::<_, std::io::Error>(NamedPipeConn(server));
                    }
                    _ = cancel_stream.cancelled() => return,
                }
            }
        }
    };

    let base_services = BaseServices {
        node_service: bundle.node_service_grpc.clone(),
        agent_session: bundle.agent_session.clone(),
        import: bundle.import.clone(),
        settings: shared.settings,
        local_agent: bundle.local_agent.clone(),
        embeddings: bundle.embeddings_service_grpc.clone(),
        database: DatabaseServiceImpl::new(manager.clone()),
    };
    build_base_router(
        Server::builder()
            .layer(TrayMetricsLayer::new(controller))
            .layer(DbManagerLayer::new(manager)),
        base_services,
    )
    .serve_with_incoming_shutdown(incoming, async move {
        combined_shutdown.await;
        cancel.cancel();
    })
    .await
    .context("gRPC server terminated with error")?;
    defuse_serve_drain_watchdog(&serve_drain_watchdog);
    drain_and_release_gpu(shutdown_manager, shared_model).await;
    Ok(())
}

/// Release the process-global GPU context after every database has been drained
/// (ADR-053: per-database compute scoping).
///
/// `DatabaseManager::shutdown_all` has already dropped each database's embedding
/// processor, so this releases the single shared NLP engine's GPU context
/// exactly once. `release_gpu_context` is one-way and global — it must never run
/// on a per-database close, only here on daemon shutdown. A short settle lets any
/// in-flight batch unwind before the model is torn down.
async fn release_shared_gpu(model: &watch::Receiver<Option<Arc<EmbeddingService>>>) {
    let Some(nlp) = model.borrow().clone() else {
        return; // no model was ever loaded
    };
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    tracing::info!("Releasing GPU context...");
    nlp.release_gpu_context();
    tracing::info!("GPU context released");
}

/// Install the shutdown signal future at boot time so a failure to register
/// the handlers becomes a startup error rather than a silent runtime fault.
///
/// On Unix we listen for SIGTERM and SIGINT. On other platforms we fall back
/// to `tokio::signal::ctrl_c`, which fails synchronously here only if the
/// platform doesn't support it.
#[cfg(unix)]
fn install_shutdown_handler() -> Result<impl std::future::Future<Output = ()>> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).context("install SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("install SIGINT handler")?;

    Ok(async move {
        tokio::select! {
            _ = sigterm.recv() => tracing::info!("SIGTERM received — initiating graceful shutdown"),
            _ = sigint.recv()  => tracing::info!("SIGINT received — initiating graceful shutdown"),
        }
    })
}

#[cfg(not(unix))]
fn install_shutdown_handler() -> Result<impl std::future::Future<Output = ()>> {
    Ok(async {
        match tokio::signal::ctrl_c().await {
            Ok(()) => tracing::info!("Ctrl-C received — initiating graceful shutdown"),
            Err(e) => tracing::error!(error = %e, "ctrl_c handler failed; shutting down"),
        }
    })
}

#[cfg(all(test, unix))]
mod uds_permission_tests {
    use super::bind_uds_owner_only;
    use std::os::unix::fs::PermissionsExt;

    fn mode_of(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    // The UDS is the local authorization boundary (ADR-052): after bind it
    // must be 0o600 regardless of what the directory's own owner-only mode
    // would otherwise have let the ambient umask produce. Deliberately does
    // NOT touch the process umask — see the note on
    // `binding_the_socket_does_not_corrupt_a_concurrently_created_directory`
    // below for why.
    #[tokio::test]
    async fn bind_uds_owner_only_is_0o600() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let sock = dir.path().join("test.sock");

        let listener = bind_uds_owner_only(&sock).expect("bind should succeed");

        assert_eq!(
            mode_of(&sock),
            0o600,
            "socket must be owner-only once bound"
        );
        drop(listener);
    }

    // The owner-only socket directory is what makes the window between `bind`
    // and the chmod harmless, so a directory anyone else can traverse must
    // fail closed rather than bind and hope (ADR-052).
    #[tokio::test]
    async fn bind_uds_owner_only_refuses_a_group_or_other_reachable_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let sock = dir.path().join("test.sock");

        let err = bind_uds_owner_only(&sock).unwrap_err().to_string();

        assert!(
            err.contains("group/other can reach"),
            "a traversable socket directory must be refused, got: {err}"
        );
        assert!(!sock.exists(), "nothing may be bound when the check fails");
    }

    /// Regression test for a flakiness class where binding the daemon's UDS
    /// must never perturb ambient-umask-governed directory creation happening
    /// concurrently elsewhere in the process. `cargo test` runs the whole
    /// suite on one process's thread pool, so the old implementation — which
    /// narrowed the process-global umask to `0o177` around `bind()` — could
    /// strip the execute bit off a directory *any other test* created via
    /// `DirBuilder`/`tempfile::tempdir()` (both default to `0o777` shaped only
    /// by the ambient umask) at that instant, leaving it unusable:
    /// `Permission denied (os error 13)` on a path the victim test had just
    /// created itself.
    ///
    /// This test does not mutate the process umask itself — doing so here
    /// would reintroduce exactly that hazard against every other test running
    /// concurrently in this binary. Instead it drives a real bind concurrently
    /// (barrier-synced, on a second OS thread) with a directory creation under
    /// whatever ambient umask this test process already has, and asserts the
    /// concurrently-created directory ends up bit-for-bit identical to one
    /// created with no bind in flight at all — i.e. the bind has zero
    /// observable effect on it. If `bind_uds_owner_only` ever narrows the
    /// process umask again, this has a real (timing-dependent, like the
    /// original bug) chance of catching it; it can never itself corrupt a
    /// sibling test's directory, unlike the mutation it guards against.
    #[tokio::test]
    async fn binding_the_socket_does_not_corrupt_a_concurrently_created_directory() {
        use std::sync::{Arc, Barrier};

        // Baseline: what a solo directory creation looks like under today's
        // ambient umask, no bind happening at all.
        let baseline_parent = tempfile::tempdir().expect("baseline tempdir");
        let baseline_dir = baseline_parent.path().join("baseline");
        std::fs::DirBuilder::new()
            .create(&baseline_dir)
            .expect("baseline mkdir");
        let expected_mode = mode_of(&baseline_dir);
        assert_eq!(
            expected_mode & 0o100,
            0o100,
            "precondition: the ambient test-runner umask must not already strip \
             the owner-execute bit, or this test can't tell corruption from baseline"
        );

        let bind_dir = tempfile::tempdir().expect("bind tempdir");
        std::fs::set_permissions(bind_dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let sock = bind_dir.path().join("concurrent.sock");

        let victim_parent = tempfile::tempdir().expect("victim tempdir");
        let victim_dir = victim_parent.path().join("victim");

        let barrier = Arc::new(Barrier::new(2));
        let victim_barrier = barrier.clone();
        let victim_dir_thread = victim_dir.clone();
        let victim_handle = std::thread::spawn(move || {
            victim_barrier.wait();
            // Mirrors tempfile::tempdir(): DirBuilder at the OS default 0o777,
            // shaped only by the ambient umask — exactly what a concurrently
            // running unrelated test creates.
            std::fs::DirBuilder::new()
                .create(&victim_dir_thread)
                .expect("victim mkdir")
        });

        barrier.wait();
        let listener = bind_uds_owner_only(&sock);
        victim_handle.join().expect("victim thread panicked");
        let listener = listener.expect("bind should succeed");

        assert_eq!(
            mode_of(&victim_dir),
            expected_mode,
            "a directory created concurrently with the UDS bind must come out \
             identical to one created with no bind in flight — a mismatch means \
             bind_uds_owner_only is once again perturbing the process-global umask"
        );

        // The literal symptom from the linked issues: prove the directory is
        // actually usable, not merely correctly-moded.
        std::fs::write(victim_dir.join("f"), b"ok")
            .expect("victim dir must remain traversable/writable after a concurrent bind");

        drop(listener);
    }
}

#[cfg(all(test, windows))]
mod pipe_permission_tests {
    use super::{create_owner_only_pipe, current_user_sid_string, owner_only_security_attributes};

    /// A pipe name unique to this test run, so concurrently running tests in
    /// this same `cargo test` process (the default) never collide on a name.
    fn unique_pipe_name(tag: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!(
            r"\\.\pipe\nodespace-daemon-test-{tag}-{}-{n}",
            std::process::id()
        )
    }

    #[test]
    fn current_user_sid_string_returns_a_real_sid() {
        let sid = current_user_sid_string().expect("must be able to read the current user's SID");
        assert!(
            sid.starts_with("S-1-"),
            "expected an SDDL SID string (S-1-...), got: {sid}"
        );
    }

    #[test]
    fn owner_only_security_attributes_builds_a_non_null_descriptor() {
        let mut sa = owner_only_security_attributes().expect("must build owner-only attrs");
        assert!(
            !sa.as_mut_ptr().is_null(),
            "SECURITY_ATTRIBUTES pointer must be non-null"
        );
    }

    // The instance that claims a pipe name must succeed with
    // `first_pipe_instance(true)` -- this is the "no squatter got here first"
    // happy path (ADR-052).
    #[tokio::test]
    async fn create_owner_only_pipe_claims_an_unused_name() {
        let name = unique_pipe_name("claim");
        let server = create_owner_only_pipe(&name, true);
        assert!(
            server.is_ok(),
            "claiming a never-before-used pipe name must succeed: {:?}",
            server.err()
        );
    }

    // The squatting scenario from the issue: something already owns the pipe
    // name (a malicious process, or -- as tested here -- this daemon's own
    // earlier instance) when a `first_pipe_instance(true)` claim is
    // attempted. That claim must fail loudly rather than silently succeed as
    // a second instance (ADR-052).
    #[tokio::test]
    async fn create_owner_only_pipe_first_instance_true_fails_against_an_already_owned_name() {
        let name = unique_pipe_name("squat");
        let _first = create_owner_only_pipe(&name, true).expect("first claim must succeed");

        let second_claim = create_owner_only_pipe(&name, true);
        assert!(
            second_claim.is_err(),
            "a second first_pipe_instance(true) claim against an already-owned \
             name must fail, not silently succeed"
        );
    }

    // Once this daemon has legitimately claimed the name, serving the next
    // client (the normal serial-reconnect flow) must keep working with
    // `first_pipe_instance(false)`.
    #[tokio::test]
    async fn create_owner_only_pipe_first_instance_false_succeeds_after_the_name_is_claimed() {
        let name = unique_pipe_name("reconnect");
        let _first = create_owner_only_pipe(&name, true).expect("first claim must succeed");

        let second_instance = create_owner_only_pipe(&name, false);
        assert!(
            second_instance.is_ok(),
            "a subsequent instance for the next client must succeed once the \
             name is legitimately owned: {:?}",
            second_instance.err()
        );
    }

    // Not just "it compiled and didn't error" -- read the security descriptor
    // actually attached to a live pipe handle back via the same Win32 API
    // `icacls`/`Get-Acl` are built on, and confirm it matches the intended
    // owner-only DACL: the current user as owner, and exactly one DACL ACE
    // (granting that same user access), with no ACE for `Everyone`,
    // `Authenticated Users`, or any other principal.
    #[tokio::test]
    async fn owner_only_dacl_is_actually_attached_to_the_live_pipe_handle() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
        use windows_sys::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
            SE_KERNEL_OBJECT,
        };
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        };

        let sid = current_user_sid_string().expect("must read current user SID");
        let name = unique_pipe_name("readback");
        let server = create_owner_only_pipe(&name, true).expect("claim must succeed");

        let sddl = unsafe {
            let handle = server.as_raw_handle() as HANDLE;
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            let status = GetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut sd,
            );
            assert_eq!(status, 0, "GetSecurityInfo must succeed on the live handle");

            let mut sddl_ptr: *mut u16 = std::ptr::null_mut();
            let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd,
                SDDL_REVISION_1,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut sddl_ptr,
                std::ptr::null_mut(),
            );
            assert_ne!(ok, 0, "converting the live descriptor to SDDL must succeed");
            let rendered = super::pwstr_to_string(sddl_ptr);
            if !sddl_ptr.is_null() {
                LocalFree(sddl_ptr as HLOCAL);
            }
            LocalFree(sd as HLOCAL);
            rendered
        };

        assert!(
            sddl.starts_with(&format!("O:{sid}")),
            "owner must be the current user, got: {sddl}"
        );
        // Windows resolves the GENERIC_ALL we asked for into the pipe (file)
        // object type's specific equivalent, FILE_ALL_ACCESS, when it
        // constructs the descriptor -- accept either spelling.
        assert!(
            sddl.contains(&format!("(A;;GA;;;{sid})"))
                || sddl.contains(&format!("(A;;FA;;;{sid})")),
            "DACL must grant the current user's SID access, got: {sddl}"
        );
        assert_eq!(
            sddl.matches("(A;").count(),
            1,
            "DACL must contain exactly one ACE (ours), got: {sddl}"
        );
        assert!(
            !sddl.contains(";;;WD)") && !sddl.contains(";;;AU)") && !sddl.contains(";;;BU)"),
            "DACL must not grant Everyone/Authenticated Users/Builtin Users \
             access, got: {sddl}"
        );
    }
}

/// The daemon must derive the same socket the desktop app dials *without*
/// `NODESPACED_SOCKET` in its environment.
///
/// The plist sets that variable, but `launchctl kickstart -k` restarts the job
/// definition launchd already has loaded rather than re-reading the plist, so a
/// daemon can outlive the variable that told it which socket to bind. When the
/// daemon's fallback was the unscoped `daemon.sock`, a Pro or dev daemon that
/// lost the variable bound a socket its own app never dialed: a healthy daemon
/// serving nobody, and an app reporting "daemon not running". Only the release
/// community build was unaffected, because that is the one variant where the
/// scoped and unscoped names coincide — which is precisely why a test that
/// checks a single variant would not have caught it.
#[cfg(all(test, unix))]
mod socket_fallback_variant_tests {
    use super::default_socket_path_for;

    /// Every variant, spelled out literally rather than re-derived from
    /// `nodespace_proto::socket`. The app side pins the identical four strings
    /// against its own resolver, so the two agree on values a change to the
    /// shared table cannot quietly move in lockstep.
    const EXPECTED: [(bool, bool, &str); 4] = [
        (false, false, ".nodespace/daemon.sock"),
        (false, true, ".nodespace/daemon-pro.sock"),
        (true, false, ".nodespace/daemon-dev.sock"),
        (true, true, ".nodespace/daemon-dev-pro.sock"),
    ];

    /// `default_socket_path_for` still reads `HOME`, which `cargo test` shares
    /// across threads, so this asserts on the suffix under whatever `HOME` the
    /// runner has rather than pinning an absolute path.
    #[test]
    fn every_variant_falls_back_to_its_own_scoped_socket() {
        for (is_debug, is_pro, expected) in EXPECTED {
            let resolved = default_socket_path_for(is_debug, is_pro);
            assert!(
                resolved.ends_with(expected),
                "variant (debug={is_debug}, pro={is_pro}) fell back to {} — \
                 expected it to end with {expected}. A daemon that binds a socket \
                 its own app does not dial is unreachable.",
                resolved.display()
            );
        }
    }

    /// The regression itself: before the fix, all four variants resolved to the
    /// community `daemon.sock`. Distinctness is what makes the fallback correct.
    #[test]
    fn variants_do_not_collapse_onto_one_socket() {
        let mut resolved: Vec<_> = EXPECTED
            .iter()
            .map(|&(d, p, _)| default_socket_path_for(d, p))
            .collect();
        resolved.sort();
        resolved.dedup();
        assert_eq!(
            resolved.len(),
            4,
            "each build variant must fall back to a distinct socket"
        );
    }

    /// `NODESPACED_SOCKET` stays an override, not a suggestion — the plist sets
    /// it, and the two-window dev setup depends on it winning over the default.
    /// Making the fallback variant-scoped must not have demoted it.
    ///
    /// This is the one test in the binary that mutates `NODESPACED_SOCKET`, so
    /// it owns that variable outright: everything else here reads only `HOME`.
    #[test]
    fn the_env_override_still_wins_over_the_scoped_default() {
        let prev = std::env::var_os("NODESPACED_SOCKET");

        std::env::set_var("NODESPACED_SOCKET", "/tmp/ns-override.sock");
        assert_eq!(
            super::socket_path(),
            std::path::PathBuf::from("/tmp/ns-override.sock"),
            "NODESPACED_SOCKET must still override the scoped default"
        );

        std::env::remove_var("NODESPACED_SOCKET");
        assert_eq!(
            super::socket_path(),
            super::default_socket_path_for(cfg!(debug_assertions), super::is_pro_build()),
            "with no override, socket_path must be exactly this build's scoped default"
        );

        match prev {
            Some(v) => std::env::set_var("NODESPACED_SOCKET", v),
            None => std::env::remove_var("NODESPACED_SOCKET"),
        }
    }
}
