//! Pro-tier sync commands invoked from the Svelte frontend.
//!
//! All commands no-op (return early with `Ok`) when the Tauri app is
//! running in community mode — i.e. there is no `ProClient` in
//! managed state. That keeps the frontend's invoke calls
//! side-effect-free when probing for sync UI.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Manager};

use crate::services::pro_client::pb::cloud_sync_service_client::CloudSyncServiceClient;
use crate::services::pro_client::pb::list_tenant_memberships_response::Selection as TenantSelection;
use crate::services::pro_client::pb::sync_status_event::State as PbState;
use crate::services::pro_client::pb::{
    AcceptInviteRequest, ActivateDatabaseRequest, ApproveRequestRequest, BindTenantRequest,
    CreateInviteRequest, EnableSyncRequest, GetIdentityRequest, InitiateOAuthRequest,
    JoinCollectionRequest, LeaveCollectionRequest, ListInvitesRequest,
    ListJoinableCollectionsRequest, ListMembersRequest, ListRequestsRequest,
    ListTenantMembershipsRequest, RemoveMemberRequest, RequestJoinRequest, RevokeInviteRequest,
    SetMemberRequest, SignOutRequest, TenantMembershipInfo, WatchSyncStatusRequest,
};
use crate::services::{ProClient, ProTier};
use tonic::transport::Channel;

/// Auth-Worker URL used for the OAuth flow — not client-configurable.
/// Release builds hit the deployed canonical domain (`pro.nodespace.ai`);
/// debug builds hit the local `wrangler dev` worker
/// (`127.0.0.1:8787`, what `device-sync.sh` runs).
#[cfg(debug_assertions)]
const DEFAULT_WORKER_URL: &str = "http://127.0.0.1:8787";
#[cfg(not(debug_assertions))]
const DEFAULT_WORKER_URL: &str = "https://pro.nodespace.ai";

/// Flag tracking whether the status-stream task is already running.
/// Module-level so repeated calls to `pro_subscribe_sync_status` from
/// the frontend (e.g. across hot-reloads) don't pile up tasks.
static STREAM_SPAWNED: AtomicBool = AtomicBool::new(false);

/// Bound on the `WatchSyncStatus` (re)subscribe so a wedged channel can't hang
/// the forwarding task outside its generation select.
const SUBSCRIBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Backoff before re-subscribing after a transient stream end/error (a channel
/// rebuild wakes the task immediately, ahead of this).
const RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

/// Snapshot of the most recent tier-detection result. Returned to
/// the frontend on demand so the UI doesn't have to wait for the
/// `pro:tier-detected` Tauri event when re-mounting.
#[tauri::command]
pub async fn pro_tier(app: AppHandle) -> Result<ProTier, String> {
    match app.try_state::<ProClient>() {
        Some(pro) => Ok(pro.tier().await),
        None => Ok(ProTier::Community),
    }
}

/// The daemon's most recent sync status (state, detail, signed-in email), cached
/// from the `WatchSyncStatus` stream. Lets the frontend re-hydrate the signed-in
/// state on demand — on a webview reload the one-shot `pro:tier-detected` event
/// (which carries the initial status) does not re-fire, and `sync:status` only
/// pushes on change, so without this a signed-in Pro user appears signed out.
/// `None` in community mode or before any status is known.
#[tauri::command]
pub async fn pro_current_status(app: AppHandle) -> Result<Option<SyncStatusSnapshot>, String> {
    let Some(pro) = app.try_state::<ProClient>() else {
        return Ok(None);
    };
    // Read both fields together under one lock acquisition (not two separate
    // active_database_id()/last_status() calls) — a database switch racing
    // between two separate reads could otherwise attribute a stale status
    // from the PREVIOUS database to the newly-active one.
    let (database_id, last_status) = pro.status_snapshot().await;
    Ok(last_status.map(|s| SyncStatusSnapshot {
        state: s.state,
        detail: s.detail,
        user_email: s.user_email,
        database_id: database_id.unwrap_or_default(),
    }))
}

/// Serializable snapshot of `SyncStatusEvent` for [`pro_current_status`].
#[derive(serde::Serialize)]
pub struct SyncStatusSnapshot {
    pub state: i32,
    pub detail: String,
    pub user_email: String,
    /// The database this status is attributed to — see
    /// [`crate::services::ProClient::active_database_id`]. Empty when no
    /// `pro_activate_database` call has landed yet.
    pub database_id: String,
}

/// Start a long-lived `WatchSyncStatus` subscription on the daemon
/// and forward each event to the frontend as a Tauri event named
/// `sync:status`.
///
/// Idempotent: only the first call spawns the task. Subsequent calls
/// return immediately.
#[tauri::command]
pub async fn pro_subscribe_sync_status(app: AppHandle) -> Result<(), String> {
    let Some(pro) = app.try_state::<ProClient>() else {
        // Community mode — nothing to subscribe to.
        return Ok(());
    };
    if STREAM_SPAWNED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    // Owned handle (Arc-backed) so the forwarding task can re-fetch the client on
    // a channel rebuild and keep the cached status current for re-hydration.
    let pro_client = pro.inner().clone();
    let app_handle = app.clone();

    // Self-healing forwarding loop: re-subscribes on the current channel whenever
    // the stream ends/errors OR the channel is rebuilt after a wedged-connection
    // recovery (`ProClient::rebind` bumps `generation`). Mirrors the node watcher —
    // without it a wedge leaves the live-status stream, and the sync pill, stuck
    // until an app restart. One task runs for the process lifetime (the
    // `STREAM_SPAWNED` guard), so it is never reset here.
    tokio::spawn(async move {
        use tokio_stream::StreamExt;
        let mut generation = pro_client.subscribe_generation();
        loop {
            // Re-fetch each iteration so a rebind (fresh channel) is picked up.
            // Bound the subscribe by a timeout: the lazy channel has no client-side
            // timeout, so a wedge landing exactly during re-subscribe would
            // otherwise hang here (outside the generation select).
            let mut client = pro_client.client().await;
            let subscribe = client.watch_sync_status(WatchSyncStatusRequest {});
            let mut stream = match tokio::time::timeout(SUBSCRIBE_TIMEOUT, subscribe).await {
                Ok(Ok(resp)) => resp.into_inner(),
                // A community daemon never implements CloudSyncService — this is
                // terminal, not transient. Retrying would wake a free-tier install
                // every couple of seconds for its whole lifetime (regression:
                // free users must be unaffected). Stop the task instead.
                Ok(Err(status)) if status.code() == tonic::Code::Unimplemented => {
                    tracing::debug!(
                        "CloudSyncService unimplemented (community daemon) — sync-status task exiting"
                    );
                    return;
                }
                Ok(Err(status)) => {
                    tracing::debug!(error = %status, "sync-status subscribe failed");
                    let database_id = pro_client.attributed_database_id().await;
                    emit_disconnected(
                        &app_handle,
                        format!("sync-status subscribe failed: {status}"),
                        database_id,
                    );
                    // Wait for a channel rebuild or a short backoff, then retry.
                    tokio::select! {
                        _ = generation.changed() => {}
                        _ = tokio::time::sleep(RETRY_BACKOFF) => {}
                    }
                    continue;
                }
                Err(_elapsed) => {
                    // Subscribe timed out — the channel may be wedged mid-resubscribe.
                    // Retry: re-fetching the client picks up a rebind.
                    tracing::warn!("sync-status subscribe timed out; retrying");
                    tokio::select! {
                        _ = generation.changed() => {}
                        _ = tokio::time::sleep(RETRY_BACKOFF) => {}
                    }
                    continue;
                }
            };

            // Forward until the stream ends/errors, or the channel is rebuilt.
            let rebuilt = loop {
                tokio::select! {
                    biased;
                    _ = generation.changed() => break true,
                    item = stream.next() => match item {
                        Some(Ok(evt)) => {
                            // Cache the latest status so a reloaded webview re-hydrates
                            // deterministically instead of appearing signed out.
                            pro_client.set_last_status(evt.clone()).await;
                            // Attribute the event to whichever database the desktop last
                            // activated. `SyncStatusEvent` itself carries no database_id
                            // (ADR-053 single-active session: the daemon runs exactly one
                            // sync session at a time, so this locally-tracked id is
                            // authoritative for "which database is this about").
                            let database_id = pro_client.attributed_database_id().await;
                            let payload = serde_json::json!({
                                "state": evt.state,
                                "detail": evt.detail,
                                "user_email": evt.user_email,
                                "database_id": database_id,
                            });
                            if let Err(e) = app_handle.emit("sync:status", payload) {
                                tracing::warn!(error = %e, "failed to emit sync:status");
                                break false;
                            }
                        }
                        Some(Err(status)) => {
                            tracing::warn!(error = %status, "sync-status stream item error");
                            break false;
                        }
                        None => break false,
                    }
                }
            };

            if rebuilt {
                // Fresh channel — re-subscribe at once, no disconnected flash.
                tracing::info!("sync-status: channel rebuilt, re-subscribing");
                continue;
            }
            // Ended/errored on the same channel: grey the pill, back off, retry —
            // waking immediately if the channel is rebuilt meanwhile.
            tracing::info!("sync-status stream ended; reconnecting");
            let database_id = pro_client.attributed_database_id().await;
            emit_disconnected(&app_handle, "sync-status stream ended".into(), database_id);
            tokio::select! {
                _ = generation.changed() => {}
                _ = tokio::time::sleep(RETRY_BACKOFF) => {}
            }
        }
    });

    Ok(())
}

/// Emit a synthetic `sync:status` event with `state =
/// STATE_DISCONNECTED` so the frontend can return the pill to its
/// grey "Sign in" baseline after the WatchSyncStatus stream ends
/// (subscription failure, daemon stream-close, item error). Without
/// this the UI keeps showing whatever state the daemon last emitted
/// and there's no signal that the stream is gone.
fn emit_disconnected(app: &AppHandle, reason: String, database_id: String) {
    let payload = serde_json::json!({
        "state": PbState::Disconnected as i32,
        "detail": reason,
        "user_email": "",
        "database_id": database_id,
    });
    if let Err(e) = app.emit("sync:status", payload) {
        tracing::warn!(error = %e, "failed to emit synthetic sync:status DISCONNECTED");
    }
}

/// Kick off the daemon's OAuth PKCE flow. The daemon opens the
/// system browser and listens on a localhost callback; this command
/// returns the attempt ID synchronously. UI tracks progress via the
/// `sync:status` stream wired in `pro_subscribe_sync_status`.
///
/// The worker URL always resolves to [`DEFAULT_WORKER_URL`] — it's not
/// client-configurable, since accepting an arbitrary URL from the
/// frontend would let it redirect the OAuth flow to an attacker-controlled
/// worker. `user_hint` is shown in the worker's login form so users see
/// which account they're signing into; empty string is fine. `provider`
/// selects a social sign-in — empty = the Worker email/password form
/// (default), `"google"` = direct Supabase GoTrue OAuth.
#[tauri::command]
pub async fn pro_initiate_oauth(
    app: AppHandle,
    user_hint: Option<String>,
    provider: Option<String>,
) -> Result<String, String> {
    let Some(pro) = app.try_state::<ProClient>() else {
        return Err("community tier — Pro sign-in unavailable".into());
    };
    let mut client = pro.client().await;
    let req = InitiateOAuthRequest {
        worker_url: DEFAULT_WORKER_URL.to_string(),
        user_hint: user_hint.unwrap_or_default(),
        provider: provider.unwrap_or_default(),
    };
    tracing::info!(worker = %req.worker_url, user_hint = %req.user_hint, provider = %req.provider, "Pro: InitiateOAuth");
    let resp = client
        .initiate_o_auth(req)
        .await
        .map_err(|e| format!("InitiateOAuth failed: {e}"))?
        .into_inner();
    Ok(resp.attempt_id)
}

/// Sign out of Pro. Tells the daemon to drop its session and wipe the
/// persisted refresh token from the OS keychain, so a restart won't auto-resume.
/// The resulting AUTH_REQUIRED transition flows back through the `sync:status`
/// stream. No-ops in community mode (no `ProClient`), matching the other Pro
/// commands' side-effect-free contract.
#[tauri::command]
pub async fn pro_signout(app: AppHandle) -> Result<(), String> {
    let Some(pro) = app.try_state::<ProClient>() else {
        return Ok(());
    };
    let mut client = pro.client().await;
    client
        .sign_out(SignOutRequest {})
        .await
        .map_err(|e| format!("SignOut failed: {e}"))?;
    tracing::info!("Pro: SignOut");
    Ok(())
}

/// Enable per-database cloud sync after the user's first-Pro consent. Tells the
/// daemon to flip the active database's `sync_enabled` flag, which unlocks the
/// collaboration UI and lets the Pro-UI variant advance from the enable prompt
/// to sign-in. No-ops in community mode (no `ProClient`), matching the other Pro
/// commands' side-effect-free contract.
#[tauri::command]
pub async fn pro_enable_sync(app: AppHandle) -> Result<(), String> {
    let Some(pro) = app.try_state::<ProClient>() else {
        return Ok(());
    };
    let mut client = pro.client().await;
    client
        .enable_sync(EnableSyncRequest {})
        .await
        .map_err(|e| format!("EnableSync failed: {e}"))?;
    tracing::info!("Pro: EnableSync");
    Ok(())
}

/// Re-target the Pro cloud-sync session to follow the newly-active database
/// (ADR-053 single-active sync). The frontend calls this right after
/// `set_active_database` re-points routing, so switching databases in the app
/// also switches which tenant syncs — not just which one is read/written. Empty
/// `database_id` deactivates (local-only). No-ops in community mode (no
/// `ProClient`), matching the other Pro commands' side-effect-free contract, so
/// the community database switcher is unaffected.
#[tauri::command]
pub async fn pro_activate_database(app: AppHandle, database_id: String) -> Result<(), String> {
    let Some(pro) = app.try_state::<ProClient>() else {
        return Ok(());
    };
    let mut client = pro.client().await;
    client
        .activate_database(ActivateDatabaseRequest {
            database_id: database_id.clone(),
        })
        .await
        .map_err(|e| format!("ActivateDatabase failed: {e}"))?;
    tracing::info!("Pro: ActivateDatabase");
    // Track which database is now the sync target so subsequent `sync:status`
    // events (which carry no `database_id` of their own — ADR-053 single-active
    // session) can be attributed to it for the frontend's per-database store.
    // Only recorded on success — a failed ActivateDatabase didn't actually
    // re-target the daemon's session, so the old attribution must stand.
    pro.set_active_database_id(database_id).await;
    Ok(())
}

// --- Add-synced-database-from-cloud (ADR-053 discovery-driven bind) -------
//
// The two RPCs a fresh-profile "add synced database" flow needs beyond
// sign-in: enumerate the signed-in user's tenants, then bind a local
// database to the chosen one. Both are thin JWT-forwarding pass-throughs,
// mirroring the membership commands below — no authority of their own.

/// One tenant the signed-in user belongs to, returned by
/// [`pro_list_tenant_memberships`].
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantMembershipDto {
    /// The tenant id (the billable unit).
    pub tenant_id: String,
    /// Tenant schema, e.g. "tenant_demo"; may be empty if not returned.
    pub schema: String,
    /// "pending" | "active" | "suspended" | "removed".
    pub status: String,
    /// "owner" | "tenant-admin" | "member" (open set); may be empty.
    pub role: String,
}

fn to_membership_dto(m: TenantMembershipInfo) -> TenantMembershipDto {
    TenantMembershipDto {
        tenant_id: m.tenant_id,
        schema: m.schema,
        status: m.status,
        role: m.role,
    }
}

/// Every tenant the signed-in user belongs to, plus the daemon's selection
/// hint, returned by [`pro_list_tenant_memberships`].
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantMembershipsDto {
    /// Every tenant the caller belongs to, with its authoritative status.
    pub memberships: Vec<TenantMembershipDto>,
    /// "no-workspace" (zero active tenants) | "auto-select" (exactly one) |
    /// "picker" (more than one) — derived from the ACTIVE memberships. The
    /// frontend drives the flow off this rather than re-deriving it, so
    /// daemon and client agree on the count.
    pub selection: String,
    /// Populated only when `selection == "auto-select"`.
    pub auto_selected: Option<TenantMembershipDto>,
}

/// List every tenant the signed-in user belongs to (discovery-driven
/// sign-in) — the tenant picker's data source for the "add synced database"
/// flow. Requires a signed-in session; the cloud RPC enumerates via
/// `public.my_tenant_memberships()`, scoped server-side to the caller's JWT.
#[tauri::command]
pub async fn pro_list_tenant_memberships(app: AppHandle) -> Result<TenantMembershipsDto, String> {
    let mut client = membership_client(&app).await?;
    let resp = client
        .list_tenant_memberships(ListTenantMembershipsRequest {})
        .await
        .map_err(|e| format!("ListTenantMemberships failed: {e}"))?
        .into_inner();

    let selection = match TenantSelection::try_from(resp.selection) {
        Ok(TenantSelection::AutoSelect) => "auto-select",
        Ok(TenantSelection::Picker) => "picker",
        // NoWorkspace and any unrecognized wire value both fail safe to the
        // same "nothing to auto-select or pick from" outcome the frontend
        // renders identically (an empty-state message).
        Ok(TenantSelection::NoWorkspace) | Err(_) => "no-workspace",
    }
    .to_string();

    Ok(TenantMembershipsDto {
        memberships: resp
            .memberships
            .into_iter()
            .map(to_membership_dto)
            .collect(),
        selection,
        auto_selected: resp.auto_selected.map(to_membership_dto),
    })
}

/// Result of [`pro_bind_tenant`] — mirrors `BindTenantResponse`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindTenantResultDto {
    /// True when the bind started a cloud-sync session (non-empty schema);
    /// false on unbind (the database is now local-only).
    pub synced: bool,
    /// The tenant schema now bound/syncing; empty when unbound.
    pub schema: String,
}

/// Bind a local database to a cloud tenant and start syncing it (ADR-053
/// per-database cloud sync) — the "add synced database" flow's final step.
/// Writes the tenant binding to the target database's settings and
/// activates it (activate-on-bind), so a freshly-created local-only database
/// becomes synced — including the cursor-0 catch-up the orchestrator runs
/// for a newly-bound database — in this single call. `database_id` empty
/// targets the active/default database; `schema` empty unbinds (goes
/// local-only); `collection` is the default landing collection within the
/// tenant, ignored when `schema` is empty.
///
/// This only binds on the daemon side — it does not change which database
/// the desktop app is *viewing*. The frontend still calls
/// `set_active_database` (and, via `databaseStore.switchTo`,
/// `pro_activate_database`) afterward to switch the UI onto the newly-bound
/// database; that second `ActivateDatabase` call is idempotent against the
/// session bind-on-activate already started.
#[tauri::command]
pub async fn pro_bind_tenant(
    app: AppHandle,
    database_id: Option<String>,
    schema: String,
    collection: Option<String>,
) -> Result<BindTenantResultDto, String> {
    let mut client = membership_client(&app).await?;
    let resp = client
        .bind_tenant(BindTenantRequest {
            database_id: database_id.unwrap_or_default(),
            schema,
            collection: collection.unwrap_or_default(),
        })
        .await
        .map_err(|e| format!("BindTenant failed: {e}"))?
        .into_inner();
    tracing::info!(schema = %resp.schema, synced = resp.synced, "Pro: BindTenant");
    Ok(BindTenantResultDto {
        synced: resp.synced,
        schema: resp.schema,
    })
}

// --- Team membership commands (M5) ----------------------------------
//
// Thin pass-throughs over the daemon's `CloudSyncService` membership RPCs. The
// daemon forwards the signed-in user's JWT to the matching cloud RPC, so the
// admin / last-admin / open-vs-restricted gates are enforced server-side — these
// commands carry no authority of their own. No collaboration UI is wired yet;
// they exist for tests and a future UI, mirroring the `pro_initiate_oauth`
// shape (resolve the Pro client or fail in community mode, then one RPC).

/// One roster entry returned by [`pro_list_members`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemberDto {
    pub person_id: String,
    /// "admin" | "modify" | "readOnly".
    pub permission: String,
}

/// One pending invite returned by [`pro_list_invites`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct InviteDto {
    /// uuid — the handle [`pro_revoke_invite`] takes.
    pub id: String,
    /// 64-hex share code (bearer); may be surfaced for the admin to copy.
    pub code: String,
    /// Bound invitee email; empty for a bearer share-code.
    pub email: String,
    /// "admin" | "modify" | "readOnly".
    pub permission: String,
    /// RFC3339; empty when the invite never expires.
    pub expires_at: String,
}

/// One pending join request returned by [`pro_list_requests`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct RequestDto {
    /// uuid — the handle [`pro_approve_request`] / [`pro_revoke_invite`] take.
    pub id: String,
    /// The requester's person_node_id.
    pub requested_by: String,
    /// RFC3339.
    pub created_at: String,
}

/// One joinable collection returned by [`pro_list_joinable_collections`] —
/// collection discovery (browse & join).
#[derive(Debug, Clone, serde::Serialize)]
pub struct JoinableCollectionDto {
    /// Collection node id.
    pub id: String,
    /// Display name (the collection node's content).
    pub name: String,
    /// `true` => needs a request (admin approval); `false` => open self-join.
    pub restricted: bool,
}

/// The caller's own identity, returned by [`pro_current_person`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct PersonDto {
    /// Bound PersonNode id; empty on an un-bound device ("role unknown").
    pub person_id: String,
    /// Signed-in email; empty when signed out.
    pub email: String,
}

/// Resolve the Pro gRPC client, or fail when running in community mode (no
/// `ProClient` in managed state). Mirrors the guard in `pro_initiate_oauth`.
async fn membership_client(app: &AppHandle) -> Result<CloudSyncServiceClient<Channel>, String> {
    match app.try_state::<ProClient>() {
        Some(pro) => Ok(pro.client().await),
        None => Err("community tier — Pro membership operations unavailable".into()),
    }
}

/// Add a member or change their role on a collection (admin only, server-gated).
/// `permission` is "admin" | "modify" | "readOnly".
#[tauri::command]
pub async fn pro_set_member(
    app: AppHandle,
    collection_id: String,
    person_id: String,
    permission: String,
) -> Result<(), String> {
    let mut client = membership_client(&app).await?;
    client
        .set_member(SetMemberRequest {
            collection_id,
            person_id,
            permission,
        })
        .await
        .map_err(|e| format!("SetMember failed: {e}"))?;
    Ok(())
}

/// Remove a member from a collection (admin only; last-admin protected server-side).
#[tauri::command]
pub async fn pro_remove_member(
    app: AppHandle,
    collection_id: String,
    person_id: String,
) -> Result<(), String> {
    let mut client = membership_client(&app).await?;
    client
        .remove_member(RemoveMemberRequest {
            collection_id,
            person_id,
        })
        .await
        .map_err(|e| format!("RemoveMember failed: {e}"))?;
    Ok(())
}

/// Leave a collection the signed-in user belongs to (resolved from their JWT).
#[tauri::command]
pub async fn pro_leave_collection(app: AppHandle, collection_id: String) -> Result<(), String> {
    let mut client = membership_client(&app).await?;
    client
        .leave_collection(LeaveCollectionRequest { collection_id })
        .await
        .map_err(|e| format!("LeaveCollection failed: {e}"))?;
    Ok(())
}

/// List the roster of a collection (member/admin).
#[tauri::command]
pub async fn pro_list_members(
    app: AppHandle,
    collection_id: String,
) -> Result<Vec<MemberDto>, String> {
    let mut client = membership_client(&app).await?;
    let resp = client
        .list_members(ListMembersRequest { collection_id })
        .await
        .map_err(|e| format!("ListMembers failed: {e}"))?
        .into_inner();
    Ok(resp
        .members
        .into_iter()
        .map(|m| MemberDto {
            person_id: m.person_id,
            permission: m.permission,
        })
        .collect())
}

/// Create an invite (admin only). Returns the invite code. When `email` is set
/// the invite is bound to it; otherwise it's a bearer share-code. `ttl_secs` of
/// `None`/`0` uses the server default (7 days).
#[tauri::command]
pub async fn pro_create_invite(
    app: AppHandle,
    collection_id: String,
    permission: String,
    email: Option<String>,
    ttl_secs: Option<u64>,
) -> Result<String, String> {
    let mut client = membership_client(&app).await?;
    let resp = client
        .create_invite(CreateInviteRequest {
            collection_id,
            permission,
            email: email.unwrap_or_default(),
            ttl_secs: ttl_secs.unwrap_or(0),
        })
        .await
        .map_err(|e| format!("CreateInvite failed: {e}"))?
        .into_inner();
    Ok(resp.code)
}

/// Redeem an invite code (invitee's own JWT). Returns the joined collection id
/// so the caller can pull it.
#[tauri::command]
pub async fn pro_accept_invite(app: AppHandle, code: String) -> Result<String, String> {
    let mut client = membership_client(&app).await?;
    let resp = client
        .accept_invite(AcceptInviteRequest { code })
        .await
        .map_err(|e| format!("AcceptInvite failed: {e}"))?
        .into_inner();
    Ok(resp.detail)
}

/// Request to join a restricted collection. Returns the request id.
#[tauri::command]
pub async fn pro_request_join(app: AppHandle, collection_id: String) -> Result<String, String> {
    let mut client = membership_client(&app).await?;
    let resp = client
        .request_join(RequestJoinRequest { collection_id })
        .await
        .map_err(|e| format!("RequestJoin failed: {e}"))?
        .into_inner();
    Ok(resp.request_id)
}

/// Self-join an OPEN collection (the complement to `pro_request_join`, which is for
/// restricted collections). The cloud RPC rejects a restricted collection, so the
/// open-vs-restricted gate is enforced server-side.
#[tauri::command]
pub async fn pro_join_collection(app: AppHandle, collection_id: String) -> Result<(), String> {
    let mut client = membership_client(&app).await?;
    client
        .join_collection(JoinCollectionRequest { collection_id })
        .await
        .map_err(|e| format!("JoinCollection failed: {e}"))?;
    Ok(())
}

/// List the collections the signed-in user could join but isn't a member of yet
/// (open + restricted) — collection discovery (browse & join). The daemon
/// forwards the caller's JWT, so the cloud RPC filters to what the caller can see
/// and excludes their own memberships server-side.
#[tauri::command]
pub async fn pro_list_joinable_collections(
    app: AppHandle,
) -> Result<Vec<JoinableCollectionDto>, String> {
    let mut client = membership_client(&app).await?;
    let resp = client
        .list_joinable_collections(ListJoinableCollectionsRequest {})
        .await
        .map_err(|e| format!("ListJoinableCollections failed: {e}"))?
        .into_inner();
    Ok(resp
        .collections
        .into_iter()
        .map(|c| JoinableCollectionDto {
            id: c.id,
            name: c.name,
            restricted: c.restricted,
        })
        .collect())
}

/// Approve a pending join request (admin only). `permission` of `None`/empty
/// grants the originally-requested tier.
#[tauri::command]
pub async fn pro_approve_request(
    app: AppHandle,
    request_id: String,
    permission: Option<String>,
) -> Result<(), String> {
    let mut client = membership_client(&app).await?;
    client
        .approve_request(ApproveRequestRequest {
            request_id,
            permission: permission.unwrap_or_default(),
        })
        .await
        .map_err(|e| format!("ApproveRequest failed: {e}"))?;
    Ok(())
}

/// List a collection's pending invites (admin only, server-gated).
#[tauri::command]
pub async fn pro_list_invites(
    app: AppHandle,
    collection_id: String,
) -> Result<Vec<InviteDto>, String> {
    let mut client = membership_client(&app).await?;
    let resp = client
        .list_invites(ListInvitesRequest { collection_id })
        .await
        .map_err(|e| format!("ListInvites failed: {e}"))?
        .into_inner();
    Ok(resp
        .invites
        .into_iter()
        .map(|i| InviteDto {
            id: i.id,
            code: i.code,
            email: i.email,
            permission: i.permission,
            expires_at: i.expires_at,
        })
        .collect())
}

/// List a collection's pending join requests (admin only, server-gated).
#[tauri::command]
pub async fn pro_list_requests(
    app: AppHandle,
    collection_id: String,
) -> Result<Vec<RequestDto>, String> {
    let mut client = membership_client(&app).await?;
    let resp = client
        .list_requests(ListRequestsRequest { collection_id })
        .await
        .map_err(|e| format!("ListRequests failed: {e}"))?
        .into_inner();
    Ok(resp
        .requests
        .into_iter()
        .map(|r| RequestDto {
            id: r.id,
            requested_by: r.requested_by,
            created_at: r.created_at,
        })
        .collect())
}

/// Revoke a pending invite or join request by id (admin only, server-gated).
#[tauri::command]
pub async fn pro_revoke_invite(app: AppHandle, invite_id: String) -> Result<(), String> {
    let mut client = membership_client(&app).await?;
    client
        .revoke_invite(RevokeInviteRequest { invite_id })
        .await
        .map_err(|e| format!("RevokeInvite failed: {e}"))?;
    Ok(())
}

/// The caller's own identity — bound PersonNode id + signed-in email.
/// Lets the UI tell which roster row is "me" and gate admin controls on the
/// caller's own per-collection role. `person_id` is empty on an un-bound device;
/// the UI treats that as "role unknown" and hides admin controls.
#[tauri::command]
pub async fn pro_current_person(app: AppHandle) -> Result<PersonDto, String> {
    let mut client = membership_client(&app).await?;
    let resp = client
        .get_identity(GetIdentityRequest {})
        .await
        .map_err(|e| format!("GetIdentity failed: {e}"))?
        .into_inner();
    Ok(PersonDto {
        person_id: resp.person_node_id,
        email: resp.email,
    })
}
