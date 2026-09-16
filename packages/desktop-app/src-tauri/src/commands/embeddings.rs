//! Tauri Commands for Topic Embeddings
//!
//! Thin gRPC proxy: all operations are forwarded to the `EmbeddingsService`
//! running in the in-process `nodespaced` server.

use crate::commands::nodes::CommandError;
use crate::services::GrpcClient;
use crate::types::Node;
use serde::{Deserialize, Serialize};
use tauri::State;

use nodespace_proto::nodespace::{
    BatchQueueEmbeddingsRequest, GetStaleCountRequest, GetStaleCountResponse,
    QueueEmbeddingRequest, RegenerateEmbeddingRequest, SearchSemanticRequest,
    TriggerBatchEmbedRequest,
};

fn grpc_err(msg: impl std::fmt::Display) -> CommandError {
    CommandError {
        message: msg.to_string(),
        code: "GRPC_ERROR".to_string(),
        details: None,
        conflict_data: None,
    }
}

fn node_from_proto(data: nodespace_proto::NodeData) -> Option<Node> {
    let created_at = match data.created_at.parse() {
        Ok(ts) => ts,
        Err(e) => {
            tracing::warn!(
                node_id = %data.id,
                raw = %data.created_at,
                error = %e,
                "Dropping node from search results: unparseable created_at timestamp"
            );
            return None;
        }
    };
    let modified_at = match data.modified_at.parse() {
        Ok(ts) => ts,
        Err(e) => {
            tracing::warn!(
                node_id = %data.id,
                raw = %data.modified_at,
                error = %e,
                "Dropping node from search results: unparseable modified_at timestamp"
            );
            return None;
        }
    };
    let properties = serde_json::from_str(&data.properties).unwrap_or_default();
    Some(Node {
        id: data.id,
        node_type: data.node_type,
        content: data.content,
        properties,
        version: data.version,
        lifecycle_status: data.lifecycle_status,
        created_at,
        modified_at,
        mentions: Vec::new(),
        mentioned_in: Vec::new(),
        title: data.title,
    })
}

/// Generate embedding for a topic node
#[tauri::command]
pub async fn generate_root_embedding(
    grpc: State<'_, GrpcClient>,
    root_id: String,
) -> Result<(), CommandError> {
    let mut client = grpc.embeddings_client().await;

    client
        .queue_embedding(QueueEmbeddingRequest { node_id: root_id })
        .await
        .map_err(|e| grpc_err(e.message()))?;

    Ok(())
}

/// Search parameters for topic/root similarity search
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRootsParams {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exact: Option<bool>,
}

/// Search root nodes by semantic similarity using vector embeddings
#[tauri::command]
pub async fn search_roots(
    grpc: State<'_, GrpcClient>,
    params: SearchRootsParams,
) -> Result<Vec<Node>, CommandError> {
    if params.query.trim().is_empty() {
        return Err(CommandError {
            message: "Query parameter cannot be empty".to_string(),
            code: "INVALID_PARAMETER".to_string(),
            details: None,
            conflict_data: None,
        });
    }

    if let Some(threshold) = params.threshold {
        if !(0.0..=1.0).contains(&threshold) {
            return Err(CommandError {
                message: "Threshold must be between 0.0 and 1.0".to_string(),
                code: "INVALID_PARAMETER".to_string(),
                details: None,
                conflict_data: None,
            });
        }
    }

    let mut client = grpc.embeddings_client().await;

    let response = client
        .search_semantic(SearchSemanticRequest {
            query: params.query,
            threshold: params.threshold.unwrap_or(0.0),
            limit: params.limit.map(|l| l as i32).unwrap_or(0),
            exact: params.exact.unwrap_or(false),
        })
        .await
        .map_err(|e| grpc_err(e.message()))?;

    let nodes = response
        .into_inner()
        .nodes
        .into_iter()
        .filter_map(node_from_proto)
        .collect();

    Ok(nodes)
}

/// Update embedding for a topic/root node immediately
#[tauri::command]
pub async fn update_root_embedding(
    grpc: State<'_, GrpcClient>,
    root_id: String,
) -> Result<(), CommandError> {
    let mut client = grpc.embeddings_client().await;

    client
        .regenerate_embedding(RegenerateEmbeddingRequest { node_id: root_id })
        .await
        .map_err(|e| grpc_err(e.message()))?;

    Ok(())
}

/// Smart trigger: Topic/root closed/unfocused
#[tauri::command]
pub async fn on_root_closed(
    grpc: State<'_, GrpcClient>,
    root_id: String,
) -> Result<(), CommandError> {
    let mut client = grpc.embeddings_client().await;

    client
        .queue_embedding(QueueEmbeddingRequest { node_id: root_id })
        .await
        .map_err(|e| grpc_err(e.message()))?;

    Ok(())
}

/// Smart trigger: Idle timeout
#[tauri::command]
pub async fn on_root_idle(
    grpc: State<'_, GrpcClient>,
    root_id: String,
) -> Result<bool, CommandError> {
    let mut client = grpc.embeddings_client().await;

    client
        .queue_embedding(QueueEmbeddingRequest { node_id: root_id })
        .await
        .map_err(|e| grpc_err(e.message()))?;

    // Returns `true` for compatibility with the frontend idle-trigger contract,
    // which expects a boolean "was queued" signal. The gRPC call already returns
    // an error on failure via the `?` above, so `true` is always correct here.
    Ok(true)
}

/// Manually sync all stale topics
#[tauri::command]
pub async fn sync_embeddings(grpc: State<'_, GrpcClient>) -> Result<(), CommandError> {
    let mut client = grpc.embeddings_client().await;

    client
        .trigger_batch_embed(TriggerBatchEmbedRequest {})
        .await
        .map_err(|e| grpc_err(e.message()))?;

    Ok(())
}

/// Map a `get_stale_count` gRPC result to the command's `Result<usize>`.
///
/// `Unimplemented` collapses to `Ok(0)`: the daemon serves no
/// `EmbeddingsService` — `nodespaced-pro` omits the NLP/embedding stack
/// (no llama-cpp) and never registers it — so there
/// is no embedding queue and zero is the honest count.
///
/// Note this arm intentionally collapses *any* `Unimplemented` for this
/// RPC, including the (today non-existent) case where a **registered**
/// `EmbeddingsService` leaves `get_stale_count` itself unimplemented at the
/// method level. That's acceptable because the community daemon implements
/// it; were that to change, the queue indicator would read zero rather than
/// surface the gap. Every other status surfaces as an error so real
/// failures still reach the UI.
///
/// Extracted as a pure fn (no `State`/network) so the
/// `Unimplemented → Ok(0)` contract is unit-testable without a mock gRPC
/// server.
fn stale_count_from_result(
    result: Result<tonic::Response<GetStaleCountResponse>, tonic::Status>,
) -> Result<usize, CommandError> {
    match result {
        Ok(response) => Ok(response.into_inner().count as usize),
        Err(status) if status.code() == tonic::Code::Unimplemented => Ok(0),
        Err(status) => Err(grpc_err(status.message())),
    }
}

/// Get count of stale topics/roots.
///
/// Degrades gracefully when the daemon serves no `EmbeddingsService` (e.g.
/// `nodespaced-pro`): the status bar polls this every 5s, so an
/// `Unimplemented` must not flood the console. See
/// [`stale_count_from_result`] for the mapping rationale.
#[tauri::command]
pub async fn get_stale_root_count(grpc: State<'_, GrpcClient>) -> Result<usize, CommandError> {
    let mut client = grpc.embeddings_client().await;
    stale_count_from_result(client.get_stale_count(GetStaleCountRequest {}).await)
}

/// Error details for a failed batch embedding operation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchEmbeddingError {
    pub root_id: String,
    pub error: String,
}

/// Result of batch embedding operation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchEmbeddingResult {
    pub success_count: usize,
    pub failed_embeddings: Vec<BatchEmbeddingError>,
}

/// Batch generate embeddings for multiple topics/roots
#[tauri::command]
pub async fn batch_generate_embeddings(
    grpc: State<'_, GrpcClient>,
    root_ids: Vec<String>,
) -> Result<BatchEmbeddingResult, CommandError> {
    let mut client = grpc.embeddings_client().await;

    let response = client
        .batch_queue_embeddings(BatchQueueEmbeddingsRequest { node_ids: root_ids })
        .await
        .map_err(|e| grpc_err(e.message()))?;

    let inner = response.into_inner();
    let failed_embeddings = inner
        .failures
        .into_iter()
        .map(|f| BatchEmbeddingError {
            root_id: f.node_id,
            error: f.error,
        })
        .collect();

    Ok(BatchEmbeddingResult {
        success_count: inner.success_count as usize,
        failed_embeddings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_node_data(title: Option<String>) -> nodespace_proto::NodeData {
        nodespace_proto::NodeData {
            id: "n1".to_string(),
            node_type: "person".to_string(),
            content: String::new(),
            properties: "{}".to_string(),
            version: 1,
            lifecycle_status: "active".to_string(),
            created_at: "2026-09-13T00:00:00Z".to_string(),
            modified_at: "2026-09-13T00:00:00Z".to_string(),
            markdown: String::new(),
            title,
        }
    }

    /// Regression test for #2547: search-result nodes go through this
    /// separate proto→`Node` conversion (distinct from
    /// `commands::nodes::proto_node_data_to_node`), which independently
    /// hardcoded `title: None` — the same class of bug, a second call site.
    #[test]
    fn node_from_proto_carries_a_present_title_through() {
        let node = node_from_proto(sample_node_data(Some("Michael Libio".to_string())))
            .expect("valid timestamps must convert");

        assert_eq!(node.title.as_deref(), Some("Michael Libio"));
    }

    #[test]
    fn node_from_proto_carries_an_absent_title_through() {
        let node = node_from_proto(sample_node_data(None)).expect("valid timestamps must convert");

        assert_eq!(node.title, None);
    }

    #[test]
    fn test_search_params_defaults() {
        let params = SearchRootsParams {
            query: "test".to_string(),
            threshold: None,
            limit: None,
            exact: None,
        };

        assert_eq!(params.threshold.unwrap_or(0.7), 0.7);
        assert_eq!(params.limit.unwrap_or(20), 20);
        assert!(!params.exact.unwrap_or(false));
    }

    #[test]
    fn test_search_params_custom() {
        let params = SearchRootsParams {
            query: "test".to_string(),
            threshold: Some(0.8),
            limit: Some(50),
            exact: Some(true),
        };

        assert_eq!(params.threshold.unwrap(), 0.8);
        assert_eq!(params.limit.unwrap(), 50);
        assert!(params.exact.unwrap());
    }

    /// Regression guard: a daemon with no
    /// `EmbeddingsService` returns `Unimplemented`, which must collapse to
    /// a quiet `Ok(0)` rather than an error the 5s poll would flood.
    #[test]
    fn stale_count_unimplemented_maps_to_zero() {
        let out =
            stale_count_from_result(Err(tonic::Status::unimplemented("no EmbeddingsService")));
        assert_eq!(out.expect("Unimplemented should map to Ok(0)"), 0);
    }

    /// A real count passes through unchanged.
    #[test]
    fn stale_count_ok_passes_through() {
        let resp = tonic::Response::new(GetStaleCountResponse { count: 7 });
        assert_eq!(
            stale_count_from_result(Ok(resp)).expect("Ok response passes through"),
            7
        );
    }

    /// Any non-`Unimplemented` status still surfaces as an error so genuine
    /// failures aren't silently swallowed as "zero queued".
    #[test]
    fn stale_count_other_status_is_error() {
        let out = stale_count_from_result(Err(tonic::Status::unavailable("daemon down")));
        assert!(
            out.is_err(),
            "non-Unimplemented status must surface as an error"
        );
    }
}
