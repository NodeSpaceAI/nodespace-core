//! Pro-tier gRPC client (`nodespace.pro.v1.CloudSyncService`).
//!
//! Used when the Tauri app talks to `nodespaced-pro` (from the
//! private `nodespace-sync` repo). The same `tonic::transport::Channel`
//! that drives `GrpcClient` is reused — one connection, two service
//! surfaces (community `nodespace.v1` + Pro `nodespace.pro.v1`).
//!
//! Capability probe: the daemon is "Pro tier" if a single
//! `WatchSyncStatus` call returns at least one event. A community
//! daemon (`nodespaced` from core) doesn't register this service and
//! returns `Status::Unimplemented`, which we surface as "community"
//! to the rest of the app.

use std::sync::Arc;
use std::time::Duration;

use nodespace_proto::with_message_limits;
use tokio::sync::{watch, RwLock};
use tonic::transport::Channel;

/// Generated bindings for `nodespace.pro.v1`. The proto file lives
/// under `proto/nodespace_pro.proto` in this crate (vendored from
/// `nodespace-sync/nodespaced-pro/proto/`).
pub mod pb {
    tonic::include_proto!("nodespace.pro.v1");
}

use pb::cloud_sync_service_client::CloudSyncServiceClient;
use pb::{SyncStatusEvent, WatchSyncStatusRequest};

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Pro-tier client + tier-detection state.
#[derive(Clone)]
pub struct ProClient {
    inner: Arc<RwLock<ProClientInner>>,
    /// Bumped by [`ProClient::rebind`] when the underlying channel is rebuilt
    /// after a wedged-connection recovery. The `WatchSyncStatus` forwarding task
    /// watches this to drop its stream and re-subscribe on the fresh channel —
    /// otherwise the live-status stream (and the sync pill) would stay wedged
    /// until an app restart. `watch::Sender` is not `Clone`, hence the `Arc`.
    generation: Arc<watch::Sender<u64>>,
}

struct ProClientInner {
    client: CloudSyncServiceClient<Channel>,
    tier: ProTier,
    last_status: Option<SyncStatusEvent>,
    /// Registry id of the database the desktop last told the daemon to make the
    /// active cloud-sync target (via `ActivateDatabase`). `None` until the first
    /// `pro_activate_database` call succeeds — e.g. immediately after the Pro
    /// probe, before the frontend has resolved which database it's showing.
    /// `SyncStatusEvent` carries no `database_id` of its own (ADR-053 single-active
    /// session: the daemon runs exactly one sync session, so this locally-tracked
    /// id is what every `WatchSyncStatus` event is attributed to when forwarded to
    /// the frontend).
    active_database_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProTier {
    /// Daemon implements `CloudSyncService` — Pro tier.
    Pro,
    /// Daemon returned `Unimplemented` on the probe — community tier.
    Community,
    /// Probe didn't complete (timeout, transport error, …). Treated
    /// as community by the UI but kept distinct for diagnostics.
    Unknown,
}

impl ProClient {
    /// Probe for the Pro service on an existing channel. The channel
    /// is shared with `GrpcClient` so both service surfaces ride the
    /// same h2 connection — opening a parallel channel here caused
    /// "Service was not ready: transport error" on subsequent calls
    /// after the probe stream was dropped.
    pub async fn probe_on_channel(channel: Channel) -> Self {
        let mut client = with_message_limits!(CloudSyncServiceClient::new(channel));
        let (tier, last_status) = probe(&mut client).await;
        tracing::info!(?tier, "Pro capability probe complete");

        Self {
            inner: Arc::new(RwLock::new(ProClientInner {
                client,
                tier,
                last_status,
                active_database_id: None,
            })),
            generation: Arc::new(watch::channel(0u64).0),
        }
    }

    /// Subscribe to channel-rebuild notifications (see [`ProClient::rebind`]).
    /// The forwarding task selects on this to re-subscribe on the fresh channel.
    pub fn subscribe_generation(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }

    pub async fn tier(&self) -> ProTier {
        self.inner.read().await.tier
    }

    pub async fn last_status(&self) -> Option<SyncStatusEvent> {
        self.inner.read().await.last_status.clone()
    }

    /// Atomic snapshot of `(active_database_id, last_status)`, read together
    /// under ONE lock acquisition. Callers that need both fields (e.g.
    /// `pro_current_status`, to attribute the cached status to a database)
    /// must use this rather than two separate `active_database_id()` /
    /// `last_status()` calls — a database switch (`set_active_database_id`)
    /// racing between two separate reads would otherwise let a stale status
    /// from the PREVIOUS database be attributed to the NEW one.
    pub async fn status_snapshot(&self) -> (Option<String>, Option<SyncStatusEvent>) {
        let inner = self.inner.read().await;
        (inner.active_database_id.clone(), inner.last_status.clone())
    }

    /// Cache the daemon's most recent status. Called from the `WatchSyncStatus`
    /// forwarding task so `last_status` tracks the live state rather than only
    /// the probe-time snapshot — this is what lets the frontend re-hydrate the
    /// signed-in state after a webview reload (the daemon only pushes on change).
    pub async fn set_last_status(&self, status: SyncStatusEvent) {
        self.inner.write().await.last_status = Some(status);
    }

    pub async fn client(&self) -> CloudSyncServiceClient<Channel> {
        self.inner.read().await.client.clone()
    }

    /// The database id last told to the daemon via `ActivateDatabase`, or `None`
    /// before the first activation. Used to attribute `sync:status` events to a
    /// database (see [`ProClientInner::active_database_id`]).
    pub async fn active_database_id(&self) -> Option<String> {
        self.inner.read().await.active_database_id.clone()
    }

    /// [`ProClient::active_database_id`], defaulted to `""` — the convention
    /// every outgoing `database_id` payload field uses (matching
    /// `ActivateDatabaseRequest`'s own "empty = no/unknown target"
    /// convention) so every emit site shares one place to change it.
    pub async fn attributed_database_id(&self) -> String {
        self.active_database_id().await.unwrap_or_default()
    }

    /// Record the database id the desktop just activated. Empty string is
    /// normalized to `None` (deactivate / local-only), mirroring
    /// `ActivateDatabaseRequest.database_id`'s "empty = deactivate" convention.
    ///
    /// Clears the cached `last_status` when this is a genuine re-target (the
    /// previously-tracked id was `Some` and differs from the new one) — a
    /// status snapshot cached for the OLD database is not valid for the new
    /// one, and attributing it to the new id (as `pro_current_status` would)
    /// is worse than reporting no snapshot: the frontend just waits for the
    /// next real `sync:status` event, which arrives correctly tagged. A
    /// first-ever activation (`None` -> `Some`) is NOT treated as a re-target
    /// and leaves `last_status` alone — that's `load()`'s first resolution
    /// declaring what the daemon most likely already has active (ADR-053: the
    /// last-active database persists across restarts), so the probe-time
    /// snapshot is still valid and a webview reload can re-hydrate the
    /// signed-in state from it deterministically instead of appearing
    /// signed out.
    ///
    /// This is a best-effort local approximation of the daemon's real
    /// session target, not a guaranteed atomic view of it: the
    /// `WatchSyncStatus` forwarding task reads/writes this same state
    /// independently (see its call sites), so a status event racing a
    /// concurrent re-target can still land tagged with the previous id in a
    /// narrow window, and a failed `ActivateDatabase` call leaves this
    /// unchanged even though the caller may have already persisted a
    /// different intended selection. Both are narrow, self-correcting via
    /// the next genuine `sync:status` event — closing them fully would need
    /// daemon-side coordination, not just local bookkeeping.
    pub async fn set_active_database_id(&self, database_id: String) {
        let next = normalize_active_database_id(database_id);
        let mut inner = self.inner.write().await;
        let is_genuine_retarget =
            inner.active_database_id.is_some() && inner.active_database_id != next;
        if is_genuine_retarget {
            inner.last_status = None;
        }
        inner.active_database_id = next;
    }

    /// Point the cached Pro client at a freshly-rebuilt channel after a
    /// wedged-connection recovery (see [`crate::services::GrpcClient::reconnect`]).
    /// The `ProClient` caches its own clone of the shared channel, so without
    /// this every subsequent `client()` call would keep riding the dead
    /// connection. Only the transport changes — the detected tier and last
    /// cached status are preserved.
    pub async fn rebind(&self, channel: Channel) {
        {
            let mut inner = self.inner.write().await;
            inner.client = with_message_limits!(CloudSyncServiceClient::new(channel));
        }
        // Bump AFTER installing the new client (and after releasing the lock) so a
        // forwarding task woken by this notification re-fetches the already-rebound
        // client — never the old one.
        self.generation.send_modify(|g| *g = g.wrapping_add(1));
    }
}

/// Single-shot probe of `WatchSyncStatus`. Returns the detected tier
/// and the first event if one arrives within the timeout.
async fn probe(client: &mut CloudSyncServiceClient<Channel>) -> (ProTier, Option<SyncStatusEvent>) {
    let probe_call = async {
        let stream = client
            .watch_sync_status(WatchSyncStatusRequest {})
            .await?
            .into_inner();
        Ok::<_, tonic::Status>(stream)
    };

    let stream_result = match tokio::time::timeout(PROBE_TIMEOUT, probe_call).await {
        Ok(Ok(s)) => s,
        Ok(Err(status)) => {
            return if status.code() == tonic::Code::Unimplemented {
                tracing::info!("CloudSyncService unimplemented — community tier");
                (ProTier::Community, None)
            } else {
                tracing::warn!(error = %status, "Pro probe returned error");
                (ProTier::Unknown, None)
            };
        }
        Err(_) => {
            tracing::warn!("Pro probe timed out — treating as community");
            return (ProTier::Unknown, None);
        }
    };

    // Pull the first event so the UI gets the current snapshot
    // immediately instead of waiting for the next transition.
    use tokio_stream::StreamExt;
    let mut stream = stream_result;
    match tokio::time::timeout(PROBE_TIMEOUT, stream.next()).await {
        Ok(Some(Ok(evt))) => {
            tracing::info!(
                state = evt.state,
                detail = %evt.detail,
                "Pro probe received first event"
            );
            (ProTier::Pro, Some(evt))
        }
        Ok(Some(Err(status))) => {
            tracing::warn!(error = %status, "Pro probe stream error");
            (ProTier::Unknown, None)
        }
        Ok(None) | Err(_) => {
            // The daemon implements the service but didn't push an
            // event before the probe timeout. Still Pro — just no
            // current snapshot. Return `None` rather than a synthetic
            // `STATE_UNSPECIFIED` event: that synthetic would
            // decode to "Sign in" on the frontend and could prompt
            // a PKCE flow the user doesn't need. The real state
            // arrives via the subsequent `WatchSyncStatus`
            // subscription.
            (ProTier::Pro, None)
        }
    }
}

/// Normalize an `ActivateDatabase` argument for storage as the tracked active
/// database id: empty string (the proto's "deactivate / local-only"
/// convention) becomes `None`, anything else is kept as-is. Extracted as a
/// pure function so the normalization is unit-testable without a live gRPC
/// channel (see [`ProClient::set_active_database_id`]).
fn normalize_active_database_id(database_id: String) -> Option<String> {
    if database_id.is_empty() {
        None
    } else {
        Some(database_id)
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_active_database_id, ProClient};
    use tonic::transport::Channel;

    #[test]
    fn empty_database_id_normalizes_to_none() {
        assert_eq!(normalize_active_database_id(String::new()), None);
    }

    #[test]
    fn non_empty_database_id_is_kept() {
        assert_eq!(
            normalize_active_database_id("db-alpha".to_string()),
            Some("db-alpha".to_string())
        );
    }

    /// A `ProClient` bound to nothing real (port 1 refuses immediately, so
    /// `probe_on_channel`'s connect attempt fails fast rather than riding out
    /// its 2s timeout) — enough to exercise `set_active_database_id` /
    /// `status_snapshot` without a live daemon.
    async fn disconnected_client() -> ProClient {
        let channel = Channel::from_static("http://127.0.0.1:1").connect_lazy();
        ProClient::probe_on_channel(channel).await
    }

    #[tokio::test]
    async fn a_genuine_retarget_clears_the_cached_status() {
        let pro = disconnected_client().await;
        pro.set_active_database_id("db-a".to_string()).await;
        pro.set_last_status(super::SyncStatusEvent {
            state: 6,
            detail: String::new(),
            user_email: "user@example.com".to_string(),
        })
        .await;
        assert!(pro.last_status().await.is_some());

        // Switching to a DIFFERENT database must drop the now-stale status —
        // it belonged to db-a, not db-b, and attributing it to the
        // newly-active database is worse than reporting no snapshot at all.
        pro.set_active_database_id("db-b".to_string()).await;

        assert_eq!(pro.active_database_id().await, Some("db-b".to_string()));
        assert!(pro.last_status().await.is_none());
    }

    #[tokio::test]
    async fn the_first_ever_activation_leaves_a_probe_time_status_alone() {
        let pro = disconnected_client().await;
        pro.set_last_status(super::SyncStatusEvent {
            state: 6,
            detail: String::new(),
            user_email: "user@example.com".to_string(),
        })
        .await;

        // First-ever activation (None -> Some) is `load()`'s first
        // resolution declaring what the daemon most likely already has
        // active, not a re-target — the probe-time snapshot must survive so
        // reload re-hydration isn't broken for the common (steady-state)
        // case.
        pro.set_active_database_id("db-a".to_string()).await;

        assert_eq!(pro.active_database_id().await, Some("db-a".to_string()));
        assert!(pro.last_status().await.is_some());
    }

    #[tokio::test]
    async fn re_activating_the_same_database_leaves_the_cached_status_alone() {
        let pro = disconnected_client().await;
        pro.set_active_database_id("db-a".to_string()).await;
        pro.set_last_status(super::SyncStatusEvent {
            state: 6,
            detail: String::new(),
            user_email: "user@example.com".to_string(),
        })
        .await;

        pro.set_active_database_id("db-a".to_string()).await;

        assert!(pro.last_status().await.is_some());
    }
}
