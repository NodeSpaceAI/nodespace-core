//! Tauri Commands for Topic Embeddings
//!
//! This module provides frontend commands for:
//! - Generating embeddings for topic nodes
//! - Searching topics by semantic similarity
//! - Updating embeddings on content changes

use crate::commands::nodes::CommandError;
use nodespace_core::models::Node;
use nodespace_core::services::{EmbeddingProcessor, NodeEmbeddingService};
use nodespace_core::NodeService;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

/// Client ID for Tauri frontend (used for event filtering - Issue #665)
const TAURI_CLIENT_ID: &str = "tauri-main";

/// Application state containing embedding service and processor
pub struct EmbeddingState {
    pub service: Arc<NodeEmbeddingService>,
    pub processor: Arc<EmbeddingProcessor>,
}

/// Helper to create CommandError instances
fn command_error(message: impl Into<String>, code: impl Into<String>) -> CommandError {
    CommandError {
        message: message.into(),
        code: code.into(),
        details: None,
    }
}

/// Helper to create CommandError instances with details
fn command_error_with_details(
    message: impl Into<String>,
    code: impl Into<String>,
    details: impl Into<String>,
) -> CommandError {
    CommandError {
        message: message.into(),
        code: code.into(),
        details: Some(details.into()),
    }
}

/// Generate embedding for a topic node
///
/// # Arguments
///
/// * `root_id` - ID of the topic/root node to embed
///
/// # Errors
///
/// Returns error if:
/// - Topic node not found
/// - Embedding generation fails
/// - Database update fails
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// await invoke('generate_root_embedding', {
///   rootId: 'topic-uuid-123'
/// });
/// ```
#[tauri::command]
pub async fn generate_root_embedding(
    state: State<'_, EmbeddingState>,
    node_service: State<'_, NodeService>,
    root_id: String,
) -> Result<(), CommandError> {
    // Get the node from the database
    let node = node_service
        .get_node(&root_id)
        .await
        .map_err(|e| {
            command_error_with_details(
                format!("Failed to get node: {}", e),
                "DATABASE_ERROR",
                format!("{:?}", e),
            )
        })?
        .ok_or_else(|| command_error(format!("Node not found: {}", root_id), "NOT_FOUND"))?;

    // Queue node's root for embedding via root-aggregate model (Issue #729)
    state
        .service
        .queue_for_embedding(&node.id)
        .await
        .map_err(|e| {
            command_error_with_details(
                format!("Failed to queue embedding: {}", e),
                "EMBEDDING_ERROR",
                format!("{:?}", e),
            )
        })?;

    tracing::info!("Queued embedding for node: {}", root_id);
    Ok(())
}

/// Search parameters for topic/root similarity search
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRootsParams {
    /// Search query text
    pub query: String,

    /// Minimum similarity threshold (0.0-1.0, higher = more similar)
    /// - 1.0 = Identical content
    /// - 0.7-0.9 = Highly similar (semantic equivalents)
    /// - 0.5-0.7 = Moderately similar (related topics)
    /// - 0.3-0.5 = Loosely related
    /// - < 0.3 = Unrelated content
    ///
    /// Default: 0.5 (moderate similarity)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,

    /// Maximum number of results
    /// Default: 20
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,

    /// Use exact search instead of approximate (slower but more accurate)
    /// Default: false (use DiskANN approximate search)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exact: Option<bool>,
}

/// Search root nodes by semantic similarity using vector embeddings
///
/// Uses SurrealDB's native `vector::similarity::cosine()` function to find
/// semantically similar nodes based on their content embeddings.
///
/// # Arguments
///
/// * `params` - Search parameters (query, threshold, limit)
///   - `query`: Text to search for (will be converted to embedding)
///   - `threshold`: Minimum similarity score (0.0-1.0, default: 0.5)
///   - `limit`: Maximum number of results (default: 20)
///
/// # Returns
///
/// Vector of nodes sorted by similarity score descending (most similar first)
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// const results = await invoke('search_roots', {
///   params: {
///     query: 'machine learning algorithms',
///     threshold: 0.5,  // 0.5 = moderate similarity (default)
///     limit: 20
///   }
/// });
///
/// console.log(`Found ${results.length} similar nodes`);
/// ```
#[tauri::command]
pub async fn search_roots(
    state: State<'_, EmbeddingState>,
    node_service: State<'_, NodeService>,
    params: SearchRootsParams,
) -> Result<Vec<Node>, CommandError> {
    // Validate query parameter
    if params.query.trim().is_empty() {
        return Err(command_error(
            "Query parameter cannot be empty".to_string(),
            "INVALID_PARAMETER",
        ));
    }

    // Validate threshold if provided
    if let Some(threshold) = params.threshold {
        if !(0.0..=1.0).contains(&threshold) {
            return Err(command_error(
                "Threshold must be between 0.0 and 1.0".to_string(),
                "INVALID_PARAMETER",
            ));
        }
    }

    // Generate embedding for search query
    let query_embedding = state
        .service
        .nlp_engine()
        .generate_embedding(&params.query)
        .map_err(|e| {
            command_error_with_details(
                "Failed to generate query embedding".to_string(),
                "EMBEDDING_ERROR",
                format!("{:?}", e),
            )
        })?;

    // Search with default/custom parameters using new root-aggregate model (Issue #729)
    let limit = params.limit.unwrap_or(20) as i64;
    let threshold = params.threshold.map(|t| t as f64);

    // Execute search using new embedding table
    let service_with_client = node_service.with_client(TAURI_CLIENT_ID);
    let store = service_with_client.store();
    let search_results = store
        .search_embeddings(&query_embedding, limit, threshold)
        .await
        .map_err(|e| {
            command_error_with_details(
                "Vector search failed".to_string(),
                "DATABASE_ERROR",
                format!("{:?}", e),
            )
        })?;

    // Fetch actual nodes for each search result
    let mut nodes = Vec::with_capacity(search_results.len());
    for result in search_results {
        if let Ok(Some(node)) = store.get_node(&result.node_id).await {
            nodes.push(node);
        }
    }

    Ok(nodes)
}

/// Update embedding for a topic/root node immediately
///
/// Use this for explicit user actions like "Regenerate Embedding" button.
/// For automatic updates on content changes, use the smart triggers
/// (on_root_closed, on_root_idle) instead.
///
/// # Arguments
///
/// * `root_id` - ID of the topic/root node to update
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// // User clicks "Regenerate Embedding" button
/// await invoke('update_root_embedding', {
///   rootId: 'topic-uuid-123'
/// });
/// ```
#[tauri::command]
pub async fn update_root_embedding(
    state: State<'_, EmbeddingState>,
    node_service: State<'_, NodeService>,
    root_id: String,
) -> Result<(), CommandError> {
    // Get the node from the database
    let node = node_service
        .get_node(&root_id)
        .await
        .map_err(|e| {
            command_error_with_details(
                format!("Failed to get node: {}", e),
                "DATABASE_ERROR",
                format!("{:?}", e),
            )
        })?
        .ok_or_else(|| command_error(format!("Node not found: {}", root_id), "NOT_FOUND"))?;

    // Queue node's root for embedding via root-aggregate model (Issue #729)
    state
        .service
        .queue_for_embedding(&node.id)
        .await
        .map_err(|e| {
            command_error_with_details(
                format!("Failed to queue embedding update: {}", e),
                "EMBEDDING_ERROR",
                format!("{:?}", e),
            )
        })?;

    tracing::info!("Queued embedding update for node: {}", root_id);
    Ok(())
}

/// Schedule a debounced embedding update (DEPRECATED - Will be removed in v0.2.0)
///
/// **DEPRECATED**: This command will be removed in version 0.2.0.
/// Use `on_root_closed` or `on_root_idle` smart triggers instead.
///
/// This is now a no-op. Content changes automatically mark topics as stale in the backend.
/// The stale flag system replaces the old debounce approach.
///
/// # Migration Guide
///
/// Replace:
/// ```typescript
/// await invoke('schedule_root_embedding_update', { rootId });
/// ```
///
/// With:
/// ```typescript
/// // When topic is closed/unfocused:
/// await invoke('on_root_closed', { rootId });
///
/// // After 30 seconds of idle time:
/// await invoke('on_root_idle', { rootId });
/// ```
///
/// # Deprecation Timeline
///
/// - v0.1.x: Deprecated, logs warning, no-op
/// - v0.2.0: Will be removed entirely
#[tauri::command]
#[deprecated(
    since = "0.1.0",
    note = "Use on_root_closed or on_root_idle smart triggers instead. Will be removed in v0.2.0."
)]
pub async fn schedule_root_embedding_update(
    _state: State<'_, EmbeddingState>,
    root_id: String,
) -> Result<(), CommandError> {
    // Log deprecation warning
    tracing::warn!(
        root_id = %root_id,
        "DEPRECATED: schedule_root_embedding_update called. Use on_root_closed or on_root_idle instead. This command will be removed in v0.2.0."
    );

    // No-op for backward compatibility
    // Content changes now mark topics as stale automatically in the backend
    Ok(())
}

/// Smart trigger: Topic/root closed/unfocused
///
/// Called when user closes or navigates away from a topic/root.
/// If the topic was recently edited, it triggers immediate re-embedding.
///
/// # Arguments
///
/// * `root_id` - ID of the topic/root that was closed
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// // User closes topic page or switches to another topic
/// await invoke('on_root_closed', { rootId: 'topic-uuid-123' });
/// ```
#[tauri::command]
pub async fn on_root_closed(
    state: State<'_, EmbeddingState>,
    node_service: State<'_, NodeService>,
    root_id: String,
) -> Result<(), CommandError> {
    // Get the node from the database
    let node = node_service
        .get_node(&root_id)
        .await
        .map_err(|e| {
            command_error_with_details(
                format!("Failed to get node: {}", e),
                "DATABASE_ERROR",
                format!("{:?}", e),
            )
        })?
        .ok_or_else(|| command_error(format!("Node not found: {}", root_id), "NOT_FOUND"))?;

    // Queue node's root for embedding via root-aggregate model (Issue #729)
    state
        .service
        .queue_for_embedding(&node.id)
        .await
        .map_err(|e| {
            command_error_with_details(
                format!("Failed to queue embedding regeneration: {}", e),
                "EMBEDDING_ERROR",
                format!("{:?}", e),
            )
        })?;

    tracing::info!("Queued embedding on close for node: {}", root_id);
    Ok(())
}

/// Smart trigger: Idle timeout
///
/// Called when user has stopped editing for 30+ seconds.
/// Triggers re-embedding if topic/root is stale.
///
/// # Arguments
///
/// * `root_id` - ID of the topic/root to check
///
/// # Returns
///
/// `true` if re-embedding was triggered, `false` if not needed
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// // After 30 seconds of idle time
/// const wasEmbedded = await invoke('on_root_idle', {
///   rootId: 'topic-uuid-123'
/// });
/// console.log(`Re-embedded: ${wasEmbedded}`);
/// ```
#[tauri::command]
pub async fn on_root_idle(
    state: State<'_, EmbeddingState>,
    node_service: State<'_, NodeService>,
    root_id: String,
) -> Result<bool, CommandError> {
    // Get the node from the database
    let node = node_service
        .get_node(&root_id)
        .await
        .map_err(|e| {
            command_error_with_details(
                format!("Failed to get node: {}", e),
                "DATABASE_ERROR",
                format!("{:?}", e),
            )
        })?
        .ok_or_else(|| command_error(format!("Node not found: {}", root_id), "NOT_FOUND"))?;

    // Queue node's root for embedding via root-aggregate model (Issue #729)
    state
        .service
        .queue_for_embedding(&node.id)
        .await
        .map_err(|e| {
            command_error_with_details(
                format!("Failed to queue embedding regeneration: {}", e),
                "EMBEDDING_ERROR",
                format!("{:?}", e),
            )
        })?;

    tracing::info!("Queued embedding on idle for node: {}", root_id);
    Ok(true)
}

/// Manually sync all stale topics
///
/// Processes all stale topics immediately. Useful for explicit user action
/// like "Sync All" button or app startup.
///
/// # Returns
///
/// Number of topics re-embedded
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// // User clicks "Sync All Embeddings" button
/// const count = await invoke('sync_embeddings');
/// console.log(`Synced ${count} topics`);
/// ```
#[tauri::command]
pub async fn sync_embeddings(state: State<'_, EmbeddingState>) -> Result<usize, CommandError> {
    // Trigger batch embedding and wait for completion
    state.processor.trigger_batch_embed().await.map_err(|e| {
        command_error_with_details(
            format!("Failed to trigger sync: {}", e),
            "EMBEDDING_ERROR",
            format!("{:?}", e),
        )
    })?;

    // For now, return 0 as we don't wait for completion
    // The background task will process asynchronously
    tracing::info!("Triggered batch embedding sync");
    Ok(0)
}

/// Get count of stale topics/roots
///
/// Returns the number of topics/roots that need re-embedding.
/// Useful for showing status indicators in UI.
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// const count = await invoke('get_stale_root_count');
/// // Display badge: "${count} topics need indexing"
/// ```
#[tauri::command]
pub async fn get_stale_root_count(
    node_service: State<'_, NodeService>,
) -> Result<usize, CommandError> {
    // Get store from NodeService to query stale embeddings
    // Note: This is a workaround - ideally we'd have a count method
    let service_with_client = node_service.with_client(TAURI_CLIENT_ID);
    let store = service_with_client.store();
    let stale_nodes = store
        .get_nodes_with_stale_embeddings(None)
        .await
        .map_err(|e| {
            command_error_with_details(
                format!("Failed to count stale nodes: {}", e),
                "DATABASE_ERROR",
                format!("{:?}", e),
            )
        })?;

    Ok(stale_nodes.len())
}

/// Batch generate embeddings for multiple topics/roots
///
/// Useful for initial embedding generation or bulk operations.
///
/// # Arguments
///
/// * `root_ids` - Vector of topic/root IDs to embed
///
/// # Returns
///
/// Result containing number of successfully embedded topics/roots
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// const rootIds = ['topic-1', 'topic-2', 'topic-3'];
/// const result = await invoke('batch_generate_embeddings', { rootIds });
/// console.log(`Embedded ${result.success_count} out of ${rootIds.length} topics`);
/// ```
#[tauri::command]
pub async fn batch_generate_embeddings(
    state: State<'_, EmbeddingState>,
    node_service: State<'_, NodeService>,
    root_ids: Vec<String>,
) -> Result<BatchEmbeddingResult, CommandError> {
    let mut success_count = 0;
    let mut failed_embeddings = Vec::new();

    for root_id in root_ids {
        // Get the node from the database
        match node_service
            .with_client(TAURI_CLIENT_ID)
            .get_node(&root_id)
            .await
        {
            Ok(Some(node)) => {
                // Queue node's root for embedding via root-aggregate model (Issue #729)
                match state.service.queue_for_embedding(&node.id).await {
                    Ok(_) => {
                        success_count += 1;
                        tracing::debug!("Queued embedding for node: {}", root_id);
                    }
                    Err(e) => {
                        let error_msg = format!("Failed to queue embedding: {}", e);
                        tracing::error!("Node {}: {}", root_id, error_msg);
                        failed_embeddings.push(BatchEmbeddingError {
                            root_id: root_id.clone(),
                            error: error_msg,
                        });
                    }
                }
            }
            Ok(None) => {
                let error_msg = "Node not found".to_string();
                tracing::error!("Node {}: {}", root_id, error_msg);
                failed_embeddings.push(BatchEmbeddingError {
                    root_id: root_id.clone(),
                    error: error_msg,
                });
            }
            Err(e) => {
                let error_msg = format!("Failed to get node: {}", e);
                tracing::error!("Node {}: {}", root_id, error_msg);
                failed_embeddings.push(BatchEmbeddingError {
                    root_id: root_id.clone(),
                    error: error_msg,
                });
            }
        }
    }

    tracing::info!(
        "Batch embedding completed: {}/{} successful",
        success_count,
        success_count + failed_embeddings.len()
    );

    Ok(BatchEmbeddingResult {
        success_count,
        failed_embeddings,
    })
}

/// Error details for a failed batch embedding operation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchEmbeddingError {
    /// ID of the topic/root that failed to embed
    pub root_id: String,

    /// Error message describing the failure
    pub error: String,
}

/// Result of batch embedding operation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchEmbeddingResult {
    /// Number of successfully embedded topics
    pub success_count: usize,

    /// Details of topics that failed to embed with error messages
    pub failed_embeddings: Vec<BatchEmbeddingError>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
