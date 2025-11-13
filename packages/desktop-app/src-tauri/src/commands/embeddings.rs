//! Tauri Commands for Topic Embeddings
//!
//! This module provides frontend commands for:
//! - Generating embeddings for topic nodes
//! - Searching topics by semantic similarity
//! - Updating embeddings on content changes

use crate::commands::nodes::CommandError;
use nodespace_core::models::Node;
use nodespace_core::services::NodeEmbeddingService;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

/// Application state containing embedding service
pub struct EmbeddingState {
    pub service: Arc<NodeEmbeddingService>,
}

/// Generate embedding for a topic node
///
/// # Arguments
///
/// * `container_id` - ID of the topic node to embed
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
/// await invoke('generate_container_embedding', {
///   containerId: 'topic-uuid-123'
/// });
/// ```
#[tauri::command]
pub async fn generate_container_embedding(
    _state: State<'_, EmbeddingState>,
    container_id: String,
) -> Result<(), CommandError> {
    tracing::warn!(
        "Embedding generation temporarily disabled (Issue #481) for container: {}",
        container_id
    );
    Ok(())
}

/// Search parameters for topic similarity search
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchContainersParams {
    /// Search query text
    pub query: String,

    /// Similarity threshold (0.0-1.0, lower = more similar)
    /// Default: 0.7
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

/// Search topics by semantic similarity
///
/// Uses Turso's native vector search (DiskANN) for fast approximate
/// nearest neighbors, or exact cosine distance for accurate results.
///
/// # Arguments
///
/// * `params` - Search parameters (query, threshold, limit, exact)
///
/// # Returns
///
/// Vector of topic nodes sorted by similarity (most similar first)
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// const results = await invoke('search_containers', {
///   params: {
///     query: 'machine learning',
///     threshold: 0.7,
///     limit: 20,
///     exact: false
///   }
/// });
///
/// console.log(`Found ${results.length} similar topics`);
/// ```
#[tauri::command]
pub async fn search_containers(
    _state: State<'_, EmbeddingState>,
    params: SearchContainersParams,
) -> Result<Vec<Node>, CommandError> {
    tracing::warn!(
        "Semantic search temporarily disabled (Issue #481) for query: {}",
        params.query
    );
    Ok(Vec::new())
}

/// Update embedding for a topic node immediately
///
/// Use this for explicit user actions like "Regenerate Embedding" button.
/// For automatic updates on content changes, use the smart triggers
/// (on_container_closed, on_container_idle) instead.
///
/// # Arguments
///
/// * `container_id` - ID of the topic node to update
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// // User clicks "Regenerate Embedding" button
/// await invoke('update_container_embedding', {
///   containerId: 'topic-uuid-123'
/// });
/// ```
#[tauri::command]
pub async fn update_container_embedding(
    _state: State<'_, EmbeddingState>,
    container_id: String,
) -> Result<(), CommandError> {
    tracing::warn!(
        "Embedding updates temporarily disabled (Issue #481) for container: {}",
        container_id
    );
    Ok(())
}

/// Schedule a debounced embedding update (DEPRECATED - Will be removed in v0.2.0)
///
/// **DEPRECATED**: This command will be removed in version 0.2.0.
/// Use `on_container_closed` or `on_container_idle` smart triggers instead.
///
/// This is now a no-op. Content changes automatically mark topics as stale in the backend.
/// The stale flag system replaces the old debounce approach.
///
/// # Migration Guide
///
/// Replace:
/// ```typescript
/// await invoke('schedule_container_embedding_update', { containerId });
/// ```
///
/// With:
/// ```typescript
/// // When topic is closed/unfocused:
/// await invoke('on_container_closed', { containerId });
///
/// // After 30 seconds of idle time:
/// await invoke('on_container_idle', { containerId });
/// ```
///
/// # Deprecation Timeline
///
/// - v0.1.x: Deprecated, logs warning, no-op
/// - v0.2.0: Will be removed entirely
#[tauri::command]
#[deprecated(
    since = "0.1.0",
    note = "Use on_container_closed or on_container_idle smart triggers instead. Will be removed in v0.2.0."
)]
pub async fn schedule_container_embedding_update(
    _state: State<'_, EmbeddingState>,
    container_id: String,
) -> Result<(), CommandError> {
    // Log deprecation warning
    tracing::warn!(
        container_id = %container_id,
        "DEPRECATED: schedule_container_embedding_update called. Use on_container_closed or on_container_idle instead. This command will be removed in v0.2.0."
    );

    // No-op for backward compatibility
    // Content changes now mark topics as stale automatically in the backend
    Ok(())
}

/// Smart trigger: Topic closed/unfocused
///
/// Called when user closes or navigates away from a topic.
/// If the topic was recently edited, it triggers immediate re-embedding.
///
/// # Arguments
///
/// * `container_id` - ID of the topic that was closed
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// // User closes topic page or switches to another topic
/// await invoke('on_container_closed', { containerId: 'topic-uuid-123' });
/// ```
#[tauri::command]
pub async fn on_container_closed(
    _state: State<'_, EmbeddingState>,
    container_id: String,
) -> Result<(), CommandError> {
    tracing::warn!(
        "Container close handler temporarily disabled (Issue #481) for container: {}",
        container_id
    );
    Ok(())
}

/// Smart trigger: Idle timeout
///
/// Called when user has stopped editing for 30+ seconds.
/// Triggers re-embedding if topic is stale.
///
/// # Arguments
///
/// * `container_id` - ID of the topic to check
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
/// const wasEmbedded = await invoke('on_container_idle', {
///   containerId: 'topic-uuid-123'
/// });
/// console.log(`Re-embedded: ${wasEmbedded}`);
/// ```
#[tauri::command]
pub async fn on_container_idle(
    _state: State<'_, EmbeddingState>,
    container_id: String,
) -> Result<bool, CommandError> {
    tracing::warn!(
        "Idle timeout handler temporarily disabled (Issue #481) for container: {}",
        container_id
    );
    Ok(false)
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
pub async fn sync_embeddings(_state: State<'_, EmbeddingState>) -> Result<usize, CommandError> {
    tracing::warn!("Embedding sync temporarily disabled (Issue #481)");
    Ok(0)
}

/// Get count of stale topics
///
/// Returns the number of topics that need re-embedding.
/// Useful for showing status indicators in UI.
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// const count = await invoke('get_stale_container_count');
/// // Display badge: "${count} topics need indexing"
/// ```
#[tauri::command]
pub async fn get_stale_container_count(
    _state: State<'_, EmbeddingState>,
) -> Result<usize, CommandError> {
    tracing::warn!("Stale container count temporarily disabled (Issue #481)");
    Ok(0)
}

/// Batch generate embeddings for multiple topics
///
/// Useful for initial embedding generation or bulk operations.
///
/// # Arguments
///
/// * `container_ids` - Vector of topic IDs to embed
///
/// # Returns
///
/// Result containing number of successfully embedded topics
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// const containerIds = ['topic-1', 'topic-2', 'topic-3'];
/// const result = await invoke('batch_generate_embeddings', { containerIds });
/// console.log(`Embedded ${result.success_count} out of ${containerIds.length} topics`);
/// ```
#[tauri::command]
pub async fn batch_generate_embeddings(
    _state: State<'_, EmbeddingState>,
    container_ids: Vec<String>,
) -> Result<BatchEmbeddingResult, CommandError> {
    tracing::warn!(
        "Batch embedding generation temporarily disabled (Issue #481) for {} containers",
        container_ids.len()
    );
    Ok(BatchEmbeddingResult {
        success_count: 0,
        failed_embeddings: Vec::new(),
    })
}

/// Error details for a failed batch embedding operation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchEmbeddingError {
    /// ID of the topic that failed to embed
    pub container_id: String,

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
        let params = SearchContainersParams {
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
        let params = SearchContainersParams {
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
