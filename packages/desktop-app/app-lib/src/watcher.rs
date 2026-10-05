//! gRPC-backed watcher that bridges `nodespaced`'s `WatchNodes` stream to the
//! Tauri frontend, routed per-window by each event's `database_id` (see
//! `window_routing::emit_routed`, which replaced the previous unconditional
//! `app.emit("node:*", ...)` broadcast to every open window).
//!
//! # Status
//!
//! This is the sole, currently-active source of `node:created` /
//! `node:updated` / `node:deleted` / `relationship:*` Tauri events — started
//! unconditionally from `lib.rs`'s setup closure via `watcher::spawn(...)`.
//! There is no in-process forwarder alongside it; the Tauri process talks to
//! `nodespaced` exclusively over gRPC, and this watcher is that seam's event
//! path.
//!
//! # Behavior
//!
//! - Opens a `WatchNodes` stream over the shared [`GrpcClient`], so the stream
//!   carries the active database's `x-ns-database-id` routing header (ADR-053)
//!   and rides the same h2 connection as every other data-plane request. The
//!   stream is opened through `GrpcClient::echo_suppressed_client`, which
//!   stamps this window's `x-ns-client-id` (ADR-026's C5 extension), so the
//!   daemon drops the echoes of writes this window made through that same
//!   client (the frontend store's own optimistic writes) before they reach
//!   this stream. Writes made through the default `GrpcClient::client` carry
//!   no client id and are delivered here like any other client's.
//! - Translates each proto `NodeEvent` to a Tauri event (id + optional
//!   node_type + originating `database_id`).
//! - On stream error or disconnection, reconnects with exponential backoff
//!   starting at 1 second and capped at 30 seconds.
//! - If the daemon refuses the active database because it requires an
//!   extension this build does not support (ADR-083 §2), logs that once at
//!   `info` and makes no further attempts until the active database changes.
//! - Re-opens the stream immediately when the active database is switched, so
//!   it streams the newly-selected database's events.
//! - Exits cleanly when the supplied cancellation token is cancelled.

use std::time::Duration;

use anyhow::{Context, Result};
use nodespace_proto::nodespace::{node_event::Event as NodeEventKind, WatchRequest};
use serde::Serialize;
use tauri::{AppHandle, Runtime};
use tokio_stream::StreamExt;
use tracing::{debug, error, info, warn};

use crate::services::GrpcClient;
use crate::window_routing::emit_routed;

/// `""` is the daemon's convention for "not database-scoped" (an event from an
/// impl not opened through the registry — see the `NodeEvent.database_id`
/// comment in `node_service.proto`).
/// `emit_routed` already treats an empty id as "no id", but callers here work
/// with owned `String`s pulled out of a payload, so this makes the intent
/// explicit at each call site rather than repeating the `is_empty()` check.
fn non_empty(id: &str) -> Option<&str> {
    (!id.is_empty()).then_some(id)
}

/// Exponential backoff bounds for reconnection attempts.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Frontend payload for `node:created` / `node:updated` / `node:deleted`.
/// `database_id` (ADR-053) lets the frontend drop events from a database it is
/// no longer viewing — a belt-and-suspenders guard against events from a watch
/// stream that was open across a database switch.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeIdPayload {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_type: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    database_id: String,
}

/// Spawn the watcher as a Tokio task. Returns immediately; the task runs
/// until `cancel_token` is cancelled or the process exits.
#[cfg(not(unix))]
pub fn spawn(
    _app: AppHandle,
    _grpc_client: GrpcClient,
    _cancel_token: tokio_util::sync::CancellationToken,
) {
    // No-op on Windows — watcher uses Unix Domain Socket transport.
}

#[cfg(unix)]
pub fn spawn(
    app: AppHandle,
    grpc_client: GrpcClient,
    cancel_token: tokio_util::sync::CancellationToken,
) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run(app, grpc_client, cancel_token).await {
            error!("Node watcher exited with error: {e:#}");
        } else {
            info!("Node watcher exited cleanly");
        }
    });
}

/// Watcher loop. Connects, streams events, and reconnects with exponential
/// backoff on any failure. Exits when `cancel_token` fires.
///
/// Generic over `Runtime` (rather than hardcoded to the real `Wry` runtime,
/// like the rest of this module's public API) SOLELY so the ADR-048
/// integration test can drive it against `tauri::test`'s `MockRuntime` — a
/// real event bus with no webview. The production `spawn` entry point above
/// still only ever instantiates this with the real runtime.
#[cfg(unix)]
pub async fn run<R: Runtime>(
    app: AppHandle<R>,
    grpc_client: GrpcClient,
    cancel_token: tokio_util::sync::CancellationToken,
) -> Result<()> {
    info!("Node watcher starting");
    // Bumped on every active-database switch (ADR-053) — a change interrupts the
    // current stream so we re-open against the newly-selected database.
    let db_changed = grpc_client.subscribe_active_database();
    watch_loop(db_changed, cancel_token, || stream_once(&app, &grpc_client)).await
}

/// What the watcher does before its next attempt, given how the last one ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pause {
    /// Sleep for the current backoff, doubling it for the next failure.
    Backoff,
    /// Sleep for the backoff after a clean stream end, which resets it first.
    ResetBackoff,
    /// The active database is refused until it changes; wait for that.
    UntilDatabaseChanges,
}

/// Whether `error` is the daemon's refusal of the active database because it
/// requires an extension this build does not support (ADR-083 §2). The
/// refusal is a gRPC status, found anywhere in the error's cause chain.
fn is_database_refusal(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<tonic::Status>().is_some_and(|status| {
            nodespace_proto::requires_extension::unsupported_extensions(status).is_some()
        })
    })
}

/// Decide the pause after an attempt that ended with `outcome`.
///
/// A refused database stays refused until the user switches database, so
/// asking again on a timer cannot succeed. Every other outcome backs off.
fn pause_after(outcome: &Result<()>) -> Pause {
    match outcome {
        Ok(()) => Pause::ResetBackoff,
        Err(e) if is_database_refusal(e) => Pause::UntilDatabaseChanges,
        Err(_) => Pause::Backoff,
    }
}

/// Run `attempt` until `cancel_token` fires, pausing between attempts as
/// [`pause_after`] decides. `db_changed` is the active-database generation
/// (`GrpcClient::subscribe_active_database`); it persists across attempts, so
/// a change that lands while an attempt is running is seen by the next wait.
#[cfg(unix)]
async fn watch_loop<A, F>(
    mut db_changed: tokio::sync::watch::Receiver<u64>,
    cancel_token: tokio_util::sync::CancellationToken,
    mut attempt: A,
) -> Result<()>
where
    A: FnMut() -> F,
    F: std::future::Future<Output = Result<()>>,
{
    let mut backoff = BACKOFF_START;
    loop {
        let pause = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => {
                info!("Node watcher received shutdown signal, exiting");
                return Ok(());
            }
            Ok(()) = db_changed.changed() => {
                // Active database switched — drop the current stream and re-open
                // immediately (backoff reset) so the new database streams at once.
                debug!("Active database switched; re-opening WatchNodes stream");
                backoff = BACKOFF_START;
                continue;
            }
            outcome = attempt() => {
                let pause = pause_after(&outcome);
                match &outcome {
                    Ok(()) => debug!("WatchNodes stream ended; reconnecting"),
                    Err(e) if pause == Pause::UntilDatabaseChanges => info!(
                        "WatchNodes refused: the active database requires an extension this \
                         build does not support ({e:#}); resubscribing when the active \
                         database changes"
                    ),
                    Err(e) => warn!(
                        "WatchNodes stream failed: {e:#}; reconnecting in {:?}",
                        backoff
                    ),
                }
                pause
            }
        };

        match pause {
            Pause::ResetBackoff => backoff = BACKOFF_START,
            Pause::Backoff | Pause::UntilDatabaseChanges => {}
        }

        if pause == Pause::UntilDatabaseChanges {
            // Wait for a database switch or shutdown only. If the client is gone
            // `changed()` errors at once; fall through to the backoff sleep (whose
            // `changed()` arm only matches a real switch) rather than spin.
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!("Node watcher cancelled while waiting for a database switch");
                    return Ok(());
                }
                switched = db_changed.changed() => {
                    if switched.is_ok() {
                        debug!("Active database switched after a refusal; reconnecting now");
                        backoff = BACKOFF_START;
                        continue;
                    }
                }
            }
        }

        // Wait for backoff, a database switch, or shutdown — whichever is first.
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!("Node watcher cancelled during backoff");
                return Ok(());
            }
            Ok(()) = db_changed.changed() => {
                debug!("Active database switched during backoff; reconnecting now");
                backoff = BACKOFF_START;
            }
            _ = tokio::time::sleep(backoff) => {
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

/// Open a single WatchNodes stream over the shared client and forward events
/// until the stream ends or errors. The client carries the active database's
/// routing header, so the daemon serves only that database's events. Returns
/// `Ok(())` on clean stream end, `Err` on transport or stream error.
#[cfg(unix)]
async fn stream_once<R: Runtime>(app: &AppHandle<R>, grpc_client: &GrpcClient) -> Result<()> {
    // The subscription must carry this window's client id so the daemon knows
    // which writes are this window's own (made via `echo_suppressed_client`)
    // and drops only those echoes.
    let mut client = grpc_client.echo_suppressed_client().await;

    let mut stream = client
        .watch_nodes(WatchRequest::default())
        .await
        .context("failed to open WatchNodes stream")?
        .into_inner();

    info!("WatchNodes stream open");

    while let Some(item) = stream.next().await {
        let event = item.context("WatchNodes stream returned an error item")?;
        forward(app, event);
    }

    Ok(())
}

/// Translate a proto `NodeEvent` into the corresponding Tauri event.
fn forward<R: Runtime>(app: &AppHandle<R>, event: nodespace_proto::nodespace::NodeEvent) {
    // The database this event originated from (ADR-053). Empty when the serving
    // impl was not opened through the registry — the frontend guard treats an
    // empty id as "always applies". Computed once here (rather than at each
    // emit call site below) since it's the same routing target for every
    // variant of this one event.
    let database_id = event.database_id;
    let target = non_empty(&database_id).map(str::to_string);
    let Some(kind) = event.event else {
        debug!("Received NodeEvent with no event variant; ignoring");
        return;
    };

    match kind {
        NodeEventKind::Created(data) => {
            let payload = NodeIdPayload {
                id: data.id,
                node_type: Some(data.node_type),
                database_id,
            };
            emit_routed(app, "node:created", &payload, target.as_deref());
        }
        NodeEventKind::Updated(data) => {
            // node:updated payload omits node_type because the frontend
            // already knows the type from its cached node.
            debug!(
                node_id = %data.id,
                node_type = %data.node_type,
                "WatchNodes → emitting node:updated"
            );
            let payload = NodeIdPayload {
                id: data.id,
                node_type: None,
                database_id,
            };
            emit_routed(app, "node:updated", &payload, target.as_deref());
        }
        NodeEventKind::Deleted(d) => {
            // node_type is required — consumers (e.g. collections sidebar)
            // apply type-aware cleanup logic for schema/collection deletions
            // without fetching the already-deleted node.
            let payload = NodeIdPayload {
                id: d.node_id,
                node_type: Some(d.node_type),
                database_id,
            };
            emit_routed(app, "node:deleted", &payload, target.as_deref());
        }
        // Relationship variants — so hierarchy changes made by any other writer
        // (another window, the CLI, an agent, a writer inside the daemon) reach
        // the frontend's reactiveStructureTree.
        // `properties` arrives JSON-encoded on the wire (proto schema is
        // stable); re-parse it here before emitting so the frontend gets a
        // real object (the `has_child` listener reads `properties.order`).
        NodeEventKind::RelationshipCreated(r) => emit_relationship(
            app,
            "relationship:created",
            r,
            database_id,
            target.as_deref(),
        ),
        NodeEventKind::RelationshipUpdated(r) => emit_relationship(
            app,
            "relationship:updated",
            r,
            database_id,
            target.as_deref(),
        ),
        NodeEventKind::RelationshipDeleted(r) => {
            let payload = RelationshipDeletedOut {
                id: r.id,
                from_id: r.from_id,
                to_id: r.to_id,
                relationship_type: r.relationship_type,
                database_id,
            };
            emit_routed(app, "relationship:deleted", &payload, target.as_deref());
        }
    }
}

/// Frontend payload for `relationship:created` / `relationship:updated`
/// (camelCase via serde rename) — the shape `tauri-sync-listener.ts` expects.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RelationshipPayloadOut {
    id: String,
    from_id: String,
    to_id: String,
    relationship_type: String,
    properties: serde_json::Value,
    #[serde(skip_serializing_if = "String::is_empty")]
    database_id: String,
}

/// Frontend payload for `relationship:deleted` (no `properties` field).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RelationshipDeletedOut {
    id: String,
    from_id: String,
    to_id: String,
    relationship_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    database_id: String,
}

fn emit_relationship<R: Runtime>(
    app: &AppHandle<R>,
    name: &str,
    r: nodespace_proto::nodespace::RelationshipPayload,
    database_id: String,
    target: Option<&str>,
) {
    // `r.properties` arrives JSON-encoded as a string on the wire so
    // the proto schema stays stable across additions to the
    // underlying `serde_json::Value`. If parsing fails, we still
    // emit the event with an empty `{}` so the frontend's
    // `has_child` listener can fall back via `Date.now()` — but
    // surface the parse failure as a warning so the silent
    // ordering-corruption case is visible in logs instead of just
    // showing up as nodes sorted to the tail of their parent.
    let props = match serde_json::from_str(&r.properties) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                rel_id = %r.id,
                rel_type = %r.relationship_type,
                error = %e,
                "Failed to parse relationship properties JSON; emitting empty object"
            );
            serde_json::Value::Object(Default::default())
        }
    };
    let payload = RelationshipPayloadOut {
        id: r.id,
        from_id: r.from_id,
        to_id: r.to_id,
        relationship_type: r.relationship_type,
        properties: props,
        database_id,
    };
    emit_routed(app, name, &payload, target);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::watch;
    use tokio_util::sync::CancellationToken;

    /// The status the daemon returns for a request routed to a database that
    /// requires an extension this build does not support.
    fn refusal() -> anyhow::Error {
        anyhow::Error::new(nodespace_proto::requires_extension::status(&[
            "fixture-ext".to_string(),
        ]))
        .context("failed to open WatchNodes stream")
    }

    fn other_failure() -> anyhow::Error {
        anyhow::Error::new(tonic::Status::unavailable("daemon restarting"))
            .context("failed to open WatchNodes stream")
    }

    #[test]
    fn only_the_refusal_waits_for_a_database_change() {
        assert_eq!(
            pause_after(&Err(refusal())),
            Pause::UntilDatabaseChanges,
            "a refusal wrapped in context is still recognised"
        );
        assert_eq!(pause_after(&Err(other_failure())), Pause::Backoff);
        assert_eq!(
            pause_after(&Err(anyhow::anyhow!("plain error"))),
            Pause::Backoff
        );
        assert_eq!(pause_after(&Ok(())), Pause::ResetBackoff);
    }

    struct Harness {
        generation: Arc<watch::Sender<u64>>,
        attempts: Arc<AtomicUsize>,
        cancel: CancellationToken,
        task: tokio::task::JoinHandle<Result<()>>,
    }

    /// Runs `watch_loop` over a fake attempt that fails with `error` and
    /// counts its calls, and an active-database generation the test bumps.
    /// `on_first` runs inside the first attempt.
    fn run_loop(
        error: fn() -> anyhow::Error,
        on_first: impl Fn(&watch::Sender<u64>) + Clone + Send + 'static,
    ) -> Harness {
        let generation = Arc::new(watch::channel(0u64).0);
        let attempts = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let (in_attempt, counted) = (generation.clone(), attempts.clone());
        let task = tokio::spawn(watch_loop(
            generation.subscribe(),
            cancel.clone(),
            move || {
                let first = counted.fetch_add(1, Ordering::SeqCst) == 0;
                let in_attempt = in_attempt.clone();
                let on_first = on_first.clone();
                async move {
                    // Runs while the attempt is in flight, not before it starts.
                    if first {
                        on_first(&in_attempt);
                    }
                    Err(error())
                }
            },
        ));
        Harness {
            generation,
            attempts,
            cancel,
            task,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn after_a_refusal_it_makes_no_attempts_until_the_database_changes() {
        let Harness {
            generation,
            attempts,
            cancel,
            task,
        } = run_loop(refusal, |_| {});

        // Far longer than the backoff cap, still one attempt.
        tokio::time::sleep(BACKOFF_MAX * 10).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        generation.send_modify(|g| *g += 1);
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            2,
            "resubscribes at once when the active database changes"
        );

        cancel.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_change_during_the_refused_attempt_is_not_missed() {
        let Harness {
            attempts,
            cancel,
            task,
            ..
        } = run_loop(refusal, |generation| generation.send_modify(|g| *g += 1));

        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 2);

        cancel.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn other_errors_keep_the_exponential_backoff() {
        let Harness {
            attempts,
            cancel,
            task,
            ..
        } = run_loop(other_failure, |_| {});

        // Attempts at 0, 1, 3, 7, 15, 31 seconds: the wait doubles.
        tokio::time::sleep(Duration::from_secs(31) + Duration::from_millis(1)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 6);

        // Then capped at 30s: next attempt at 61s, not before.
        tokio::time::sleep(Duration::from_secs(29)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 6);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 7);

        cancel.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_while_waiting_after_a_refusal_exits() {
        let Harness { cancel, task, .. } = run_loop(refusal, |_| {});
        tokio::time::sleep(Duration::from_millis(1)).await;
        cancel.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_closed_active_database_channel_backs_off_instead_of_spinning() {
        let (switch, db_changed) = watch::channel(0u64);
        drop(switch);
        let attempts = Arc::new(AtomicUsize::new(0));
        let counted = attempts.clone();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(watch_loop(db_changed, cancel.clone(), move || {
            counted.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Err::<(), _>(refusal()))
        }));

        // Attempts at 0, 1 and 3 seconds.
        tokio::time::sleep(Duration::from_secs(3) + Duration::from_millis(1)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 3);

        cancel.cancel();
        task.await.unwrap().unwrap();
    }
}
