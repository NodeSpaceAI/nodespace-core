//! Tauri Commands for Topic Embeddings
//!
//! This module provides frontend commands for:
//! - Generating embeddings for topic nodes
//! - Searching topics by semantic similarity
//! - Updating embeddings on content changes

use crate::commands::nodes::CommandError;
use nodespace_core::models::Node;
use nodespace_core::services::TopicEmbeddingService;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

/// Application state containing embedding service
pub struct EmbeddingState {
    pub service: Arc<TopicEmbeddingService>,
}

/// Generate embedding for a topic node
///
/// # Arguments
///
/// * `topic_id` - ID of the topic node to embed
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
/// await invoke('generate_topic_embedding', {
///   topicId: 'topic-uuid-123'
/// });
/// ```
#[tauri::command]
pub async fn generate_topic_embedding(
    state: State<'_, EmbeddingState>,
    topic_id: String,
) -> Result<(), CommandError> {
    state
        .service
        .embed_topic(&topic_id)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to generate embedding: {}", e),
            code: "EMBEDDING_GENERATION_FAILED".to_string(),
            details: Some(format!("{:?}", e)),
        })?;

    Ok(())
}

/// Search parameters for topic similarity search
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchTopicsParams {
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
/// const results = await invoke('search_topics', {
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
pub async fn search_topics(
    state: State<'_, EmbeddingState>,
    params: SearchTopicsParams,
) -> Result<Vec<Node>, CommandError> {
    let threshold = params.threshold.unwrap_or(0.7);
    let limit = params.limit.unwrap_or(20);
    let exact = params.exact.unwrap_or(false);

    let results = if exact {
        state
            .service
            .exact_search_topics(&params.query, threshold, limit)
            .await
    } else {
        state
            .service
            .search_topics(&params.query, threshold, limit)
            .await
    };

    results.map_err(|e| CommandError {
        message: format!("Failed to search topics: {}", e),
        code: "TOPIC_SEARCH_FAILED".to_string(),
        details: Some(format!("{:?}", e)),
    })
}

/// Update embedding for a topic node immediately
///
/// Use this for explicit user actions like "Regenerate Embedding" button.
/// For automatic updates on content changes, use the smart triggers
/// (on_topic_closed, on_topic_idle) instead.
///
/// # Arguments
///
/// * `topic_id` - ID of the topic node to update
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// // User clicks "Regenerate Embedding" button
/// await invoke('update_topic_embedding', {
///   topicId: 'topic-uuid-123'
/// });
/// ```
#[tauri::command]
pub async fn update_topic_embedding(
    state: State<'_, EmbeddingState>,
    topic_id: String,
) -> Result<(), CommandError> {
    // Re-embed and mark as fresh
    state
        .service
        .embed_topic(&topic_id)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to update embedding: {}", e),
            code: "EMBEDDING_UPDATE_FAILED".to_string(),
            details: Some(format!("{:?}", e)),
        })?;

    Ok(())
}

/// Schedule a debounced embedding update (DEPRECATED - Will be removed in v0.2.0)
///
/// **DEPRECATED**: This command will be removed in version 0.2.0.
/// Use `on_topic_closed` or `on_topic_idle` smart triggers instead.
///
/// This is now a no-op. Content changes automatically mark topics as stale in the backend.
/// The stale flag system replaces the old debounce approach.
///
/// # Migration Guide
///
/// Replace:
/// ```typescript
/// await invoke('schedule_topic_embedding_update', { topicId });
/// ```
///
/// With:
/// ```typescript
/// // When topic is closed/unfocused:
/// await invoke('on_topic_closed', { topicId });
///
/// // After 30 seconds of idle time:
/// await invoke('on_topic_idle', { topicId });
/// ```
///
/// # Deprecation Timeline
///
/// - v0.1.x: Deprecated, logs warning, no-op
/// - v0.2.0: Will be removed entirely
#[tauri::command]
#[deprecated(
    since = "0.1.0",
    note = "Use on_topic_closed or on_topic_idle smart triggers instead. Will be removed in v0.2.0."
)]
pub async fn schedule_topic_embedding_update(
    _state: State<'_, EmbeddingState>,
    topic_id: String,
) -> Result<(), CommandError> {
    // Log deprecation warning
    tracing::warn!(
        topic_id = %topic_id,
        "DEPRECATED: schedule_topic_embedding_update called. Use on_topic_closed or on_topic_idle instead. This command will be removed in v0.2.0."
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
/// * `topic_id` - ID of the topic that was closed
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// // User closes topic page or switches to another topic
/// await invoke('on_topic_closed', { topicId: 'topic-uuid-123' });
/// ```
#[tauri::command]
pub async fn on_topic_closed(
    state: State<'_, EmbeddingState>,
    topic_id: String,
) -> Result<(), CommandError> {
    state
        .service
        .on_topic_closed(&topic_id)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to process topic close: {}", e),
            code: "TOPIC_CLOSE_FAILED".to_string(),
            details: Some(format!("{:?}", e)),
        })?;

    Ok(())
}

/// Smart trigger: Idle timeout
///
/// Called when user has stopped editing for 30+ seconds.
/// Triggers re-embedding if topic is stale.
///
/// # Arguments
///
/// * `topic_id` - ID of the topic to check
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
/// const wasEmbedded = await invoke('on_topic_idle', {
///   topicId: 'topic-uuid-123'
/// });
/// console.log(`Re-embedded: ${wasEmbedded}`);
/// ```
#[tauri::command]
pub async fn on_topic_idle(
    state: State<'_, EmbeddingState>,
    topic_id: String,
) -> Result<bool, CommandError> {
    state
        .service
        .on_idle_timeout(&topic_id)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to process idle timeout: {}", e),
            code: "IDLE_TIMEOUT_FAILED".to_string(),
            details: Some(format!("{:?}", e)),
        })
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
    state
        .service
        .sync_all_stale_topics()
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to sync embeddings: {}", e),
            code: "SYNC_FAILED".to_string(),
            details: Some(format!("{:?}", e)),
        })
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
/// const count = await invoke('get_stale_topic_count');
/// // Display badge: "${count} topics need indexing"
/// ```
#[tauri::command]
pub async fn get_stale_topic_count(
    state: State<'_, EmbeddingState>,
) -> Result<usize, CommandError> {
    let topics = state
        .service
        .get_all_stale_topics()
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to get stale count: {}", e),
            code: "STALE_COUNT_FAILED".to_string(),
            details: Some(format!("{:?}", e)),
        })?;
    Ok(topics.len())
}

/// Batch generate embeddings for multiple topics
///
/// Useful for initial embedding generation or bulk operations.
///
/// # Arguments
///
/// * `topic_ids` - Vector of topic IDs to embed
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
/// const topicIds = ['topic-1', 'topic-2', 'topic-3'];
/// const result = await invoke('batch_generate_embeddings', { topicIds });
/// console.log(`Embedded ${result.success_count} out of ${topicIds.length} topics`);
/// ```
#[tauri::command]
pub async fn batch_generate_embeddings(
    state: State<'_, EmbeddingState>,
    topic_ids: Vec<String>,
) -> Result<BatchEmbeddingResult, CommandError> {
    let mut success_count = 0;
    let mut failed_embeddings = Vec::new();

    for topic_id in topic_ids {
        match state.service.embed_topic(&topic_id).await {
            Ok(()) => success_count += 1,
            Err(e) => {
                tracing::error!("Failed to embed topic {}: {}", topic_id, e);
                failed_embeddings.push(BatchEmbeddingError {
                    topic_id: topic_id.clone(),
                    error: e.to_string(),
                });
            }
        }
    }

    Ok(BatchEmbeddingResult {
        success_count,
        failed_embeddings,
    })
}

/// Error details for a failed batch embedding operation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchEmbeddingError {
    /// ID of the topic that failed to embed
    pub topic_id: String,

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
        let params = SearchTopicsParams {
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
        let params = SearchTopicsParams {
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
