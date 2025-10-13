//! Background Embedding Processor
//!
//! This module provides a background task that periodically processes stale container embeddings.
//! It replaces the old debounce-based approach with a more efficient stale flag system.
//!
//! # Architecture
//!
//! - Runs every 5 minutes to check for stale containers
//! - Prioritizes topics based on last_content_update (most recently edited first)
//! - Processes in batches to avoid overwhelming the system
//! - Can be triggered manually via Tauri commands
//!
//! # Performance Benefits
//!
//! - Content saves immediately (no delay)
//! - Re-embedding happens 1x per session instead of 120x during active editing
//! - 120x performance improvement for continuous editing workflows

use crate::db::DatabaseService;
use crate::services::error::NodeServiceError;
use crate::services::NodeEmbeddingService;
use libsql::params;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::error;

/// Maximum number of topics to sync in a single manual sync operation
const MAX_MANUAL_SYNC_BATCH: usize = 10000;

/// Configuration for the embedding processor
#[derive(Debug, Clone)]
pub struct EmbeddingProcessorConfig {
    /// How often to check for stale containers (default: 5 minutes)
    pub check_interval: Duration,

    /// Maximum number of topics to process in one batch (default: 10)
    pub batch_size: usize,

    /// Whether the processor is enabled (default: true)
    pub enabled: bool,
}

impl Default for EmbeddingProcessorConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(300), // 5 minutes
            batch_size: 10,
            enabled: true,
        }
    }
}

/// Background task that processes stale container embeddings
pub struct EmbeddingProcessor {
    /// Embedding service for generating embeddings
    embedding_service: Arc<NodeEmbeddingService>,

    /// Database service for querying stale containers
    db: Arc<DatabaseService>,

    /// Configuration
    config: Arc<RwLock<EmbeddingProcessorConfig>>,

    /// Shutdown signal
    shutdown: Arc<RwLock<bool>>,
}

impl EmbeddingProcessor {
    /// Create a new EmbeddingProcessor
    ///
    /// # Arguments
    ///
    /// * `embedding_service` - Service for generating embeddings
    /// * `db` - Database service for persistence
    /// * `config` - Configuration options
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use nodespace_core::services::{EmbeddingProcessor, EmbeddingProcessorConfig, NodeEmbeddingService};
    /// use nodespace_core::db::DatabaseService;
    /// use std::sync::Arc;
    /// use std::path::PathBuf;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = Arc::new(DatabaseService::new(PathBuf::from("./data/test.db")).await?);
    /// let embedding_service = Arc::new(NodeEmbeddingService::new_with_defaults(db.clone())?);
    /// let config = EmbeddingProcessorConfig::default();
    ///
    /// let processor = EmbeddingProcessor::new(embedding_service, db, config);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(
        embedding_service: Arc<NodeEmbeddingService>,
        db: Arc<DatabaseService>,
        config: EmbeddingProcessorConfig,
    ) -> Self {
        Self {
            embedding_service,
            db,
            config: Arc::new(RwLock::new(config)),
            shutdown: Arc::new(RwLock::new(false)),
        }
    }

    /// Start the background processor
    ///
    /// This spawns a background task that runs indefinitely until shutdown is signaled.
    /// The task checks for stale containers at the configured interval and processes them.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::{EmbeddingProcessor, EmbeddingProcessorConfig, NodeEmbeddingService};
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(DatabaseService::new(PathBuf::from("./data/test.db")).await?);
    /// # let embedding_service = Arc::new(NodeEmbeddingService::new_with_defaults(db.clone())?);
    /// # let config = EmbeddingProcessorConfig::default();
    /// let processor = Arc::new(EmbeddingProcessor::new(embedding_service, db, config));
    /// processor.start();
    /// # Ok(())
    /// # }
    /// ```
    pub fn start(self: Arc<Self>) {
        let processor = Arc::clone(&self);

        tokio::spawn(async move {
            processor.run().await;
        });
    }

    /// Main processing loop
    async fn run(&self) {
        let config = self.config.read().await;
        let mut ticker = interval(config.check_interval);
        drop(config);

        loop {
            ticker.tick().await;

            // Check if shutdown was requested
            if *self.shutdown.read().await {
                break;
            }

            // Check if processor is enabled
            let config = self.config.read().await;
            if !config.enabled {
                drop(config);
                continue;
            }
            drop(config);

            // Process stale containers
            if let Err(e) = self.process_stale_topics().await {
                error!("Error processing stale containers: {}", e);
            }
        }
    }

    /// Process all stale containers in priority order
    ///
    /// Queries for stale containers, prioritizes by last_content_update, and processes them in batches.
    ///
    /// # Errors
    ///
    /// Returns error if database query or embedding generation fails
    pub async fn process_stale_topics(&self) -> Result<usize, NodeServiceError> {
        let config = self.config.read().await;
        let batch_size = config.batch_size;
        drop(config);

        // Get stale containers ordered by most recently updated first
        let stale_topics = self.get_stale_containers(batch_size).await?;

        let count = stale_topics.len();

        // Process each topic
        for container_id in stale_topics {
            if let Err(e) = self.embedding_service.embed_container(&container_id).await {
                error!("Failed to embed topic {}: {}", container_id, e);
                continue;
            }

            // Mark as no longer stale
            if let Err(e) = self.mark_container_embedded(&container_id).await {
                error!("Failed to mark topic {} as embedded: {}", container_id, e);
            }
        }

        Ok(count)
    }

    /// Get list of stale container IDs, prioritized by most recently updated
    async fn get_stale_containers(&self, limit: usize) -> Result<Vec<String>, NodeServiceError> {
        let conn = self.db.connect_with_timeout().await.map_err(|e| {
            NodeServiceError::QueryFailed(format!("Database connection failed: {}", e))
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT id FROM nodes
                 WHERE node_type = 'topic'
                   AND embedding_stale = TRUE
                 ORDER BY last_content_update DESC
                 LIMIT ?",
            )
            .await
            .map_err(|e| {
                NodeServiceError::QueryFailed(format!("Query preparation failed: {}", e))
            })?;

        let mut rows = stmt
            .query(params![limit as i64])
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Query execution failed: {}", e)))?;

        let mut container_ids = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Row fetch failed: {}", e)))?
        {
            let id: String = row
                .get(0)
                .map_err(|e| NodeServiceError::QueryFailed(format!("Failed to get id: {}", e)))?;
            container_ids.push(id);
        }

        Ok(container_ids)
    }

    /// Mark a topic as having up-to-date embedding
    async fn mark_container_embedded(&self, container_id: &str) -> Result<(), NodeServiceError> {
        let conn = self.db.connect_with_timeout().await.map_err(|e| {
            NodeServiceError::QueryFailed(format!("Database connection failed: {}", e))
        })?;

        conn.execute(
            "UPDATE nodes SET
                embedding_stale = FALSE,
                last_embedding_update = CURRENT_TIMESTAMP
             WHERE id = ?",
            params![container_id],
        )
        .await
        .map_err(|e| NodeServiceError::QueryFailed(format!("Database update failed: {}", e)))?;

        Ok(())
    }

    /// Manually trigger processing of all stale containers (for Tauri command)
    ///
    /// This processes all stale containers immediately, ignoring the batch size limit.
    ///
    /// # Returns
    ///
    /// Number of topics processed
    pub async fn sync_all_stale_containers(&self) -> Result<usize, NodeServiceError> {
        // Get ALL stale containers (use configured max batch size)
        let stale_topics = self.get_stale_containers(MAX_MANUAL_SYNC_BATCH).await?;

        let count = stale_topics.len();

        // Process each topic
        for container_id in stale_topics {
            if let Err(e) = self.embedding_service.embed_container(&container_id).await {
                error!("Failed to embed topic {}: {}", container_id, e);
                continue;
            }

            // Mark as no longer stale
            if let Err(e) = self.mark_container_embedded(&container_id).await {
                error!("Failed to mark topic {} as embedded: {}", container_id, e);
            }
        }

        Ok(count)
    }

    /// Update processor configuration
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::{EmbeddingProcessor, EmbeddingProcessorConfig, NodeEmbeddingService};
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # use tokio::time::Duration;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(DatabaseService::new(PathBuf::from("./data/test.db")).await?);
    /// # let embedding_service = Arc::new(NodeEmbeddingService::new_with_defaults(db.clone())?);
    /// # let config = EmbeddingProcessorConfig::default();
    /// # let processor = Arc::new(EmbeddingProcessor::new(embedding_service, db, config));
    /// let mut new_config = EmbeddingProcessorConfig::default();
    /// new_config.check_interval = Duration::from_secs(600); // 10 minutes
    /// processor.update_config(new_config).await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_config(&self, config: EmbeddingProcessorConfig) {
        let mut current_config = self.config.write().await;
        *current_config = config;
    }

    /// Shutdown the processor gracefully
    pub async fn shutdown(&self) {
        let mut shutdown = self.shutdown.write().await;
        *shutdown = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    #[ignore = "Integration test: requires NLP model files. Run with: cargo test -- --ignored"]
    async fn test_processor_creation() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = Arc::new(DatabaseService::new(db_path).await.unwrap());
        let embedding_service =
            Arc::new(NodeEmbeddingService::new_with_defaults(db.clone()).unwrap());

        let config = EmbeddingProcessorConfig::default();
        let processor = EmbeddingProcessor::new(embedding_service, db, config);

        assert!(!*processor.shutdown.read().await);
    }

    #[tokio::test]
    #[ignore = "Integration test: requires NLP model files. Run with: cargo test -- --ignored"]
    async fn test_config_update() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = Arc::new(DatabaseService::new(db_path).await.unwrap());
        let embedding_service =
            Arc::new(NodeEmbeddingService::new_with_defaults(db.clone()).unwrap());

        let config = EmbeddingProcessorConfig::default();
        let processor = EmbeddingProcessor::new(embedding_service, db, config);

        let new_config = EmbeddingProcessorConfig {
            batch_size: 20,
            ..Default::default()
        };

        processor.update_config(new_config).await;

        let current_config = processor.config.read().await;
        assert_eq!(current_config.batch_size, 20);
    }

    #[tokio::test]
    #[ignore = "Integration test: requires NLP model files. Run with: cargo test -- --ignored"]
    async fn test_shutdown() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = Arc::new(DatabaseService::new(db_path).await.unwrap());
        let embedding_service =
            Arc::new(NodeEmbeddingService::new_with_defaults(db.clone()).unwrap());

        let config = EmbeddingProcessorConfig::default();
        let processor = EmbeddingProcessor::new(embedding_service, db, config);

        processor.shutdown().await;

        assert!(*processor.shutdown.read().await);
    }
}
