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

/// Update embedding for a topic node (immediate, no debouncing)
///
/// Use this for explicit user actions like "Regenerate Embedding" button.
/// For automatic updates on content changes, the backend handles debouncing
/// internally via `schedule_update_topic_embedding`.
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
    state
        .service
        .update_topic_embedding(&topic_id)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to update embedding: {}", e),
            code: "EMBEDDING_UPDATE_FAILED".to_string(),
            details: Some(format!("{:?}", e)),
        })?;

    Ok(())
}

/// Schedule a debounced embedding update (for content change events)
///
/// This schedules a re-embedding after 5 seconds of inactivity.
/// If additional content changes occur within 5 seconds, the timer resets.
///
/// # Arguments
///
/// * `topic_id` - ID of the topic node that changed
///
/// # Example (from frontend)
///
/// ```typescript
/// import { invoke } from '@tauri-apps/api/tauri';
///
/// // User types in topic content
/// function onContentChange(topicId: string) {
///   // This will debounce - only re-embeds after 5 seconds of no changes
///   invoke('schedule_topic_embedding_update', { topicId });
/// }
/// ```
#[tauri::command]
pub async fn schedule_topic_embedding_update(
    state: State<'_, EmbeddingState>,
    topic_id: String,
) -> Result<(), CommandError> {
    state
        .service
        .schedule_update_topic_embedding(&topic_id)
        .await;

    Ok(())
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
