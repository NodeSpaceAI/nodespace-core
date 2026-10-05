//! Pending seed update commands (ADR-094 §8): the shipped changes to built-in
//! items the user has edited, and the choice that settles one.
//!
//! All commands proxy through the in-process gRPC server (nodespace-daemon)
//! instead of calling `packages/core` directly. Nothing here applies a
//! shipped version on its own: `resolve_pending_seed_update` is called only
//! from an explicit user action.

use nodespace_proto::nodespace::{
    ListPendingSeedUpdatesRequest, PendingSeedUpdateRef, ResolvePendingSeedUpdateRequest,
    SeedUpdateChoice,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use tonic::Request;

use super::nodes::{status_to_command_error, CommandError};
use crate::services::GrpcClient;

/// One pending update, as returned to the frontend.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSeedUpdate {
    pub node_id: String,
    /// The seeded node's type: the kind of item.
    pub node_type: String,
    pub title: String,
    /// `config` or `guidance`.
    pub aspect: String,
    pub shipped_version: String,
    pub recorded_at: String,
    pub last_edited_at: String,
    /// Whether this build holds the shipped version to show and take.
    pub shipped_available: bool,
}

impl From<nodespace_proto::nodespace::PendingSeedUpdate> for PendingSeedUpdate {
    fn from(u: nodespace_proto::nodespace::PendingSeedUpdate) -> Self {
        Self {
            node_id: u.node_id,
            node_type: u.node_type,
            title: u.title,
            aspect: u.aspect,
            shipped_version: u.shipped_version,
            recorded_at: u.recorded_at,
            last_edited_at: u.last_edited_at,
            shipped_available: u.shipped_available,
        }
    }
}

/// Both versions of one pending aspect, as text.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSeedUpdateDetail {
    pub shipped: String,
    pub yours: String,
}

/// What the user chose for a pending update.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedUpdateResolution {
    KeepMine,
    TakeShipped,
}

#[tauri::command]
pub async fn list_pending_seed_updates(
    client: State<'_, GrpcClient>,
) -> Result<Vec<PendingSeedUpdate>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .list_pending_seed_updates(Request::new(ListPendingSeedUpdatesRequest {}))
        .await
        .map_err(status_to_command_error)?
        .into_inner();
    Ok(resp.updates.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub async fn get_pending_seed_update(
    client: State<'_, GrpcClient>,
    node_id: String,
    aspect: String,
) -> Result<PendingSeedUpdateDetail, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_pending_seed_update(Request::new(PendingSeedUpdateRef { node_id, aspect }))
        .await
        .map_err(status_to_command_error)?
        .into_inner();
    Ok(PendingSeedUpdateDetail {
        shipped: resp.shipped,
        yours: resp.yours,
    })
}

/// Settle one pending update. **User-initiated only**: `take_shipped`
/// replaces the user's version of that aspect, so the frontend calls this
/// only from an explicit action, never on its own.
#[tauri::command]
pub async fn resolve_pending_seed_update(
    client: State<'_, GrpcClient>,
    node_id: String,
    aspect: String,
    choice: SeedUpdateResolution,
) -> Result<(), CommandError> {
    let choice = match choice {
        SeedUpdateResolution::KeepMine => SeedUpdateChoice::KeepMine,
        SeedUpdateResolution::TakeShipped => SeedUpdateChoice::TakeShipped,
    };
    let mut c = client.client().await;
    c.resolve_pending_seed_update(Request::new(ResolvePendingSeedUpdateRequest {
        node_id,
        aspect,
        choice: choice as i32,
    }))
    .await
    .map_err(status_to_command_error)?;
    Ok(())
}
