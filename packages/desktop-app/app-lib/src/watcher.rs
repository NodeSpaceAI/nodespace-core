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
    let mut db_changed = grpc_client.subscribe_active_database();

    let mut backoff = BACKOFF_START;
    loop {
        tokio::select! {
            biased;
            _ = cancel_token.cancelled() => {
                info!("Node watcher received shutdown signal, exiting");
                return Ok(());
            }
            _ = db_changed.changed() => {
                // Active database switched — drop the current stream and re-open
                // immediately (backoff reset) so the new database streams at once.
                debug!("Active database switched; re-opening WatchNodes stream");
                backoff = BACKOFF_START;
                continue;
            }
            outcome = stream_once(&app, &grpc_client) => {
                match outcome {
                    Ok(()) => {
                        // Server closed the stream cleanly — reconnect immediately
                        // with the backoff reset, since this isn't an error condition.
                        debug!("WatchNodes stream ended; reconnecting");
                        backoff = BACKOFF_START;
                    }
                    Err(e) => {
                        warn!("WatchNodes stream failed: {e:#}; reconnecting in {:?}", backoff);
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
            _ = db_changed.changed() => {
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
