//! Conflict-journal commands (ADR-068): list, per-node lookup, and resolution
//! for the local-only conflict journal that replaces `_possible_duplicate`.
//!
//! All commands proxy through the in-process gRPC server (nodespace-daemon)
//! instead of calling `packages/core` directly.

use nodespace_proto::nodespace::{
    ConflictsForNodeRequest, ListConflictsRequest, ResolveConflictRequest,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use tonic::Request;

use super::nodes::{status_to_command_error, CommandError};
use crate::services::GrpcClient;

/// One conflict record, as returned to the frontend. `detail`/`resolution`
/// are re-parsed from the wire's JSON-encoded strings into real objects so
/// the frontend never hand-parses a nested JSON string.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictRecord {
    pub id: String,
    pub kind: String,
    pub node_ids: Vec<String>,
    pub detail: serde_json::Value,
    pub status: String,
    pub detected_at: String,
    pub detected_by: Option<String>,
    pub occurrences: i64,
    pub last_seen_at: String,
    pub resolved_at: Option<String>,
    pub resolution: Option<serde_json::Value>,
}

fn proto_to_conflict_record(
    r: nodespace_proto::nodespace::ConflictRecord,
) -> Result<ConflictRecord, CommandError> {
    let detail = serde_json::from_str(&r.detail).map_err(|e| CommandError {
        message: format!("Failed to parse conflict detail: {e}"),
        code: "PARSE_ERROR".to_string(),
        details: None,
        conflict_data: None,
    })?;
    let resolution = r
        .resolution
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| CommandError {
            message: format!("Failed to parse conflict resolution: {e}"),
            code: "PARSE_ERROR".to_string(),
            details: None,
            conflict_data: None,
        })?;

    Ok(ConflictRecord {
        id: r.id,
        kind: r.kind,
        node_ids: r.node_ids,
        detail,
        status: r.status,
        detected_at: r.detected_at,
        detected_by: r.detected_by,
        occurrences: r.occurrences,
        last_seen_at: r.last_seen_at,
        resolved_at: r.resolved_at,
        resolution,
    })
}

/// List conflict records, optionally filtered by status/kind.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListConflictsInput {
    pub status: Option<String>,
    pub kind: Option<String>,
    pub limit: Option<i32>,
}

#[tauri::command]
pub async fn list_conflicts(
    client: State<'_, GrpcClient>,
    input: ListConflictsInput,
) -> Result<Vec<ConflictRecord>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .list_conflicts(Request::new(ListConflictsRequest {
            status: input.status,
            kind: input.kind,
            limit: input.limit.unwrap_or(0),
        }))
        .await
        .map_err(status_to_command_error)?
        .into_inner();

    resp.conflicts
        .into_iter()
        .map(proto_to_conflict_record)
        .collect()
}

/// Every conflict record naming `node_id` as a participant — backs the
/// inline "is this node in a conflict" indicator (a derived journal read,
/// never a stored property).
#[tauri::command]
pub async fn conflicts_for_node(
    client: State<'_, GrpcClient>,
    node_id: String,
) -> Result<Vec<ConflictRecord>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .conflicts_for_node(Request::new(ConflictsForNodeRequest { node_id }))
        .await
        .map_err(status_to_command_error)?
        .into_inner();

    resp.conflicts
        .into_iter()
        .map(proto_to_conflict_record)
        .collect()
}

/// Apply a resolution to a conflict record. `resolution` is the JSON-encoded
/// `Resolution` shape (`{"action":"dismiss"}`, `{"action":"adopt_existing","adopted":"<id>"}`,
/// `{"action":"rename",...}`, `{"action":"restore",...}`, `{"action":"merge",...}`).
#[tauri::command]
pub async fn resolve_conflict(
    client: State<'_, GrpcClient>,
    conflict_id: String,
    resolution: serde_json::Value,
) -> Result<ConflictRecord, CommandError> {
    let mut c = client.client().await;
    let resolution_json = serde_json::to_string(&resolution).map_err(|e| CommandError {
        message: format!("Failed to serialize resolution: {e}"),
        code: "SERIALIZE_ERROR".to_string(),
        details: None,
        conflict_data: None,
    })?;

    let resp = c
        .resolve_conflict(Request::new(ResolveConflictRequest {
            conflict_id,
            resolution: resolution_json,
        }))
        .await
        .map_err(status_to_command_error)?
        .into_inner();

    let record = resp.conflict.ok_or_else(|| CommandError {
        message: "resolve_conflict returned no conflict record".to_string(),
        code: "INTERNAL_ERROR".to_string(),
        details: None,
        conflict_data: None,
    })?;
    proto_to_conflict_record(record)
}
