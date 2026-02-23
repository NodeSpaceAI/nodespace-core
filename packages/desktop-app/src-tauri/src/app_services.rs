//! Centralized application services container with interior mutability.
//!
//! `AppServices` wraps all runtime services (database, node service, embeddings)
//! behind `Arc<RwLock<>>` so they can be hot-swapped at runtime when the user
//! switches databases — without restarting the entire application.
//!
//! Registered as a single Tauri managed state via `app.manage(AppServices::new())`.
//! All commands access services through `State<'_, AppServices>`.

use crate::commands::nodes::CommandError;
use crate::config::AppConfig;
use nodespace_core::services::{EmbeddingProcessor, NodeEmbeddingService};
use nodespace_core::{NodeService, SurrealStore};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// Application state containing embedding service and processor.
///
/// Moved from `commands/embeddings.rs` to centralize in AppServices.
pub struct EmbeddingState {
    pub service: Arc<NodeEmbeddingService>,
    pub processor: Arc<EmbeddingProcessor>,
}

/// Active services that are initialized after database connection.
///
/// All fields are populated during `init_services()` and can be
/// replaced atomically during `switch_database()`.
struct ActiveServices {
    store: Arc<SurrealStore>,
    node_service: Arc<NodeService>,
    embedding_state: Option<EmbeddingState>,
    config: AppConfig,
}

/// Centralized services container with interior mutability for hot-swapping.
///
/// Registered as Tauri managed state. Commands access services via accessor methods
/// that return `Result<Arc<T>, CommandError>` — returning a clear error if services
/// aren't initialized yet.
pub struct AppServices {
    inner: Arc<RwLock<Option<ActiveServices>>>,
    /// Per-session cancellation token for background tasks (MCP, domain event forwarder).
    /// Cancelled and replaced on each `switch_database()` call.
    session_token: Arc<RwLock<Option<CancellationToken>>>,
}

impl Default for AppServices {
    fn default() -> Self {
        Self::new()
    }
}

impl AppServices {
    /// Create an empty container. Services are populated later via `initialize()`.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            session_token: Arc::new(RwLock::new(None)),
        }
    }

    /// Get the NodeService, or error if not yet initialized.
    pub async fn node_service(&self) -> Result<Arc<NodeService>, CommandError> {
        let guard = self.inner.read().await;
        guard
            .as_ref()
            .map(|s| s.node_service.clone())
            .ok_or_else(|| CommandError {
                message: "Database not initialized. Please wait for startup to complete."
                    .to_string(),
                code: "NOT_INITIALIZED".to_string(),
                details: None,
            })
    }

    /// Get the SurrealStore, or error if not yet initialized.
    pub async fn store(&self) -> Result<Arc<SurrealStore>, CommandError> {
        let guard = self.inner.read().await;
        guard
            .as_ref()
            .map(|s| s.store.clone())
            .ok_or_else(|| CommandError {
                message: "Database not initialized. Please wait for startup to complete."
                    .to_string(),
                code: "NOT_INITIALIZED".to_string(),
                details: None,
            })
    }

    /// Get the EmbeddingState, or error if not initialized or embeddings unavailable.
    pub async fn embedding_state(
        &self,
    ) -> Result<(Arc<NodeEmbeddingService>, Arc<EmbeddingProcessor>), CommandError> {
        let guard = self.inner.read().await;
        let active = guard.as_ref().ok_or_else(|| CommandError {
            message: "Database not initialized. Please wait for startup to complete.".to_string(),
            code: "NOT_INITIALIZED".to_string(),
            details: None,
        })?;

        match &active.embedding_state {
            Some(es) => Ok((es.service.clone(), es.processor.clone())),
            None => Err(CommandError {
                message: "Embedding service not available. Model may have failed to load."
                    .to_string(),
                code: "EMBEDDINGS_UNAVAILABLE".to_string(),
                details: None,
            }),
        }
    }

    /// Get just the embedding service Arc (for MCP integration).
    pub async fn embedding_service(&self) -> Result<Arc<NodeEmbeddingService>, CommandError> {
        self.embedding_state().await.map(|(svc, _)| svc)
    }

    /// Get the AppConfig, or error if not yet initialized.
    pub async fn config(&self) -> Result<AppConfig, CommandError> {
        let guard = self.inner.read().await;
        guard
            .as_ref()
            .map(|s| s.config.clone())
            .ok_or_else(|| CommandError {
                message: "Database not initialized. Please wait for startup to complete."
                    .to_string(),
                code: "NOT_INITIALIZED".to_string(),
                details: None,
            })
    }

    /// Check whether services have been initialized.
    pub async fn is_initialized(&self) -> bool {
        self.inner.read().await.is_some()
    }

    /// Populate the container with initialized services.
    ///
    /// Called from `db.rs::init_services()` after database and services are ready.
    pub async fn initialize(
        &self,
        store: Arc<SurrealStore>,
        node_service: Arc<NodeService>,
        embedding_state: Option<EmbeddingState>,
        config: AppConfig,
        session_cancel_token: CancellationToken,
    ) {
        {
            let mut guard = self.inner.write().await;
            *guard = Some(ActiveServices {
                store,
                node_service,
                embedding_state,
                config,
            });
        }
        {
            let mut token_guard = self.session_token.write().await;
            *token_guard = Some(session_cancel_token);
        }
    }

    /// Hot-swap database: cancel session tasks, replace services, restart background tasks.
    ///
    /// The drain-then-replace protocol:
    /// 1. Cancel the current session token (stops MCP server, domain event forwarder)
    /// 2. Brief pause for background tasks to exit
    /// 3. Release GPU resources from old embedding state
    /// 4. Replace inner services with new ones
    /// 5. Set new session token
    /// 6. Caller restarts background tasks with new services
    pub async fn switch_database(
        &self,
        store: Arc<SurrealStore>,
        node_service: Arc<NodeService>,
        embedding_state: Option<EmbeddingState>,
        config: AppConfig,
        new_session_token: CancellationToken,
    ) {
        // Step 1: Cancel current session token
        {
            let token_guard = self.session_token.read().await;
            if let Some(token) = token_guard.as_ref() {
                token.cancel();
            }
        }

        // Step 2: Drain old embedding processor before releasing GPU.
        // The processor's background task shuts down when its Arc is dropped
        // (dropping the shutdown channel sender). We take ownership here so the
        // old processor stops before we release the GPU context it depends on.
        let old_embedding_state = {
            let mut guard = self.inner.write().await;
            guard
                .as_mut()
                .and_then(|active| active.embedding_state.take())
        };

        // Brief pause for background tasks (MCP, forwarder, processor) to exit
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Step 3: Release GPU resources from old embedding state.
        // The processor Arc should be the last reference — dropping it shuts down
        // the background task. Then we can safely release the GPU context.
        if let Some(old_es) = old_embedding_state {
            // Drop processor first to stop its background task
            let old_service = old_es.service;
            drop(old_es.processor);
            // Brief pause for processor task to exit
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            tracing::info!("Releasing GPU context from old embedding state...");
            old_service.nlp_engine().release_gpu_context();
            tracing::info!("GPU context released successfully");
        }

        // Step 4: Replace services
        {
            let mut guard = self.inner.write().await;
            *guard = Some(ActiveServices {
                store,
                node_service,
                embedding_state,
                config,
            });
        }

        // Step 5: Set new session token
        {
            let mut token_guard = self.session_token.write().await;
            *token_guard = Some(new_session_token);
        }
    }

    /// Get the current session cancellation token (for background task coordination).
    pub async fn session_token(&self) -> Option<CancellationToken> {
        self.session_token.read().await.clone()
    }

    /// Release GPU resources. Called during graceful shutdown.
    ///
    /// Mirrors the drain-then-release protocol from `switch_database()`:
    /// 1. Take ownership of embedding state (removes from ActiveServices)
    /// 2. Drop processor first — closes shutdown channel, background task exits
    /// 3. Brief pause for processor task to exit
    /// 4. Release GPU context (now safe — no background tasks hold references)
    pub async fn release_gpu_resources(&self) {
        // Step 1: Take ownership of embedding state (drops from ActiveServices)
        let old_embedding_state = {
            let mut guard = self.inner.write().await;
            guard
                .as_mut()
                .and_then(|active| active.embedding_state.take())
        };

        if let Some(old_es) = old_embedding_state {
            // Step 2: Drop processor first — closes shutdown channel, background task exits
            let old_service = old_es.service;
            drop(old_es.processor);

            // Step 3: Brief pause for processor task to exit
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Step 4: Now safe to release GPU context
            tracing::info!("Releasing GPU context to prevent Metal crash...");
            old_service.nlp_engine().release_gpu_context();
            tracing::info!("GPU context released successfully");
        }
    }
}
