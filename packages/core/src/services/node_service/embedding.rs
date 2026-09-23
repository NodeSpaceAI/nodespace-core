//! Embedding-related operations for NodeService.

use super::*;

impl NodeService {
    /// Read all locally-stored embedding records for a node (one per chunk).
    ///
    /// Read-only and **independent of the `nlp` feature**: it queries the
    /// persisted `embedding` table, which exists whether or not embedding
    /// *generation* (llama-cpp) is compiled in. The Pro daemon uses this to
    /// mirror a node's vectors into Supabase pgvector.
    pub async fn get_embeddings(
        &self,
        node_id: &str,
    ) -> Result<Vec<crate::models::Embedding>, NodeServiceError> {
        self.store.get_embeddings(node_id).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to read embeddings: {}", e))
        })
    }

    /// Read embedding records modified at or after `since`, across all nodes,
    /// ordered by `modified_at`. Drives the Pro daemon's cloud-push sweep:
    /// it advances a cursor over `modified_at` and pushes newly (re)computed
    /// vectors. Also independent of the `nlp` feature.
    pub async fn embeddings_modified_since(
        &self,
        since: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::models::Embedding>, NodeServiceError> {
        self.store
            .embeddings_modified_since(since)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to read embeddings since cursor: {}",
                    e
                ))
            })
    }

    /// Replace a node's embeddings with locally-generated vectors (`origin =
    /// 'local'`, wholesale). **Independent of the `nlp` feature** (it's a plain
    /// store write). Empty `embeddings` is a no-op (use [`Self::delete_embeddings`]
    /// to clear).
    pub async fn upsert_embeddings(
        &self,
        node_id: &str,
        embeddings: Vec<crate::models::NewEmbedding>,
    ) -> Result<(), NodeServiceError> {
        self.store
            .upsert_embeddings(node_id, embeddings)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to upsert embeddings: {}", e))
            })
    }

    /// Apply embeddings PULLED from another device (`origin = 'remote'`).
    /// The Pro daemon's cloud pull uses this instead of
    /// [`Self::upsert_embeddings`] so the push sweep won't re-push a vector this
    /// device merely received. Also independent of the `nlp` feature.
    pub async fn apply_remote_embeddings(
        &self,
        node_id: &str,
        embeddings: Vec<crate::models::NewEmbedding>,
    ) -> Result<(), NodeServiceError> {
        self.store
            .apply_remote_embeddings(node_id, embeddings)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to apply remote embeddings: {}", e))
            })
    }

    /// Delete all of a node's embeddings. Used by the Pro daemon's cloud pull to
    /// apply a remote embeddings delete. Also independent of the `nlp` feature.
    pub async fn delete_embeddings(&self, node_id: &str) -> Result<(), NodeServiceError> {
        self.store.delete_embeddings(node_id).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to delete embeddings: {}", e))
        })
    }

    /// Set the embedding waker for event-driven processing.
    ///
    /// Silently ignored if called more than once. Works on `Arc<NodeService>`
    /// since the waker lock is shared via `Arc`.
    #[cfg(feature = "nlp")]
    pub fn set_embedding_waker(&self, waker: crate::services::EmbeddingWaker) {
        let _ = self.embedding_waker.set(waker);
    }

    /// Resolve the *embedding root* of `node_id`: its tree root.
    ///
    /// Only a root is ever embedded, and its embedding aggregates its
    /// descendants. A child has no meaning outside its root, so it never
    /// carries an embedding of its own — including a child of a
    /// non-embeddable root such as a `date` page or a `task`.
    pub async fn get_embedding_root_id(&self, node_id: &str) -> Result<String, NodeServiceError> {
        let mut current = node_id.to_string();
        while let Some(pid) = self
            .store
            .get_parent_id(&current)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            current = pid;
        }
        Ok(current)
    }

    /// Queue a node's root for embedding regeneration
    ///
    /// Finds the root of the given node and marks its embedding as stale.
    /// Used when any node in a tree is created, updated, or deleted to ensure
    /// the root-aggregate embedding stays current.
    ///
    /// This is a non-blocking operation - errors are logged but don't fail the caller.
    #[cfg(feature = "nlp")]
    pub async fn queue_root_for_embedding(&self, node_id: &str) {
        // Find the embedding root (tree root) of this node.
        let root_id = match self.get_embedding_root_id(node_id).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    "Failed to find root for node {} (embedding not queued): {}",
                    node_id,
                    e
                );
                return;
            }
        };

        // Get root node type to check if it's embeddable (optimized - no full node fetch)
        let root_type = match self.store.get_node_type(&root_id).await {
            Ok(Some(node_type)) => node_type,
            Ok(None) => {
                tracing::warn!("Root node {} not found (embedding not queued)", root_id);
                return;
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to get root node type {} (embedding not queued): {}",
                    root_id,
                    e
                );
                return;
            }
        };

        // Only queue if root is an embeddable type
        if !self.is_embeddable_type(&root_type) {
            tracing::debug!(
                "Root {} is not embeddable (type: {}), skipping embedding queue",
                root_id,
                root_type
            );
            return;
        }

        // Check if embedding exists for this root
        let has_embedding = match self.store.has_embeddings(&root_id).await {
            Ok(has) => has,
            Err(e) => {
                tracing::warn!(
                    "Failed to check embeddings for root {} (assuming none exist): {}",
                    root_id,
                    e
                );
                false
            }
        };

        // Mark existing embedding as stale or create new stale marker
        let result = if has_embedding {
            self.store.mark_root_embedding_stale(&root_id).await
        } else {
            self.store.create_stale_embedding_marker(&root_id).await
        };

        if let Err(e) = result {
            tracing::warn!(
                "Failed to queue root {} for embedding (via node {}): {}",
                root_id,
                node_id,
                e
            );
        } else {
            tracing::debug!(
                "📥 Queued root {} for embedding (triggered by node {})",
                root_id,
                node_id
            );

            // Wake the embedding processor (fire-and-forget)
            if let Some(waker) = self.embedding_waker.get() {
                tracing::debug!("🔔 Waking embedding processor for root {}", root_id);
                waker.wake();
            } else {
                tracing::debug!(
                    "Embedding waker not yet configured — root {} will be processed on next wake",
                    root_id
                );
            }
        }
    }

    /// Static async version of queue_root_for_embedding for use in spawned tasks
    ///
    /// This is used when we want to fire-and-forget the embedding queue operation
    /// without blocking the calling thread (e.g., during node updates).
    #[cfg(feature = "nlp")]
    pub(crate) async fn queue_root_for_embedding_async(
        store: &std::sync::Arc<crate::db::SqliteStore>,
        behaviors: &std::sync::Arc<crate::behaviors::NodeBehaviorRegistry>,
        node_id: &str,
        embedding_waker: Option<&crate::services::EmbeddingWaker>,
    ) {
        // Find the embedding root — the tree root, as in
        // `NodeService::get_embedding_root_id`.
        let root_id = {
            let mut current_id = node_id.to_string();
            loop {
                match store.get_parent_id(&current_id).await {
                    Ok(Some(pid)) => current_id = pid,
                    Ok(None) => break current_id,
                    Err(e) => {
                        tracing::warn!(
                            "Failed to find root for node {} (embedding not queued): {}",
                            node_id,
                            e
                        );
                        return;
                    }
                }
            }
        };

        // Get root node type to check if it's embeddable (optimized - no full node fetch)
        let root_type = match store.get_node_type(&root_id).await {
            Ok(Some(node_type)) => node_type,
            Ok(None) => {
                tracing::warn!("Root node {} not found (embedding not queued)", root_id);
                return;
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to get root node type {} (embedding not queued): {}",
                    root_id,
                    e
                );
                return;
            }
        };

        // Only queue if root is an embeddable type (behavior-driven)
        let behavior: std::sync::Arc<dyn crate::behaviors::NodeBehavior> =
            behaviors.get(&root_type).unwrap_or_else(|| {
                std::sync::Arc::new(crate::behaviors::CustomNodeBehavior::new(&root_type))
            });
        let probe = Node {
            id: "probe".to_string(),
            node_type: root_type.clone(),
            content: "probe".to_string(),
            version: 1,
            properties: serde_json::json!({}),
            mentions: vec![],
            mentioned_in: vec![],
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            title: None,
            lifecycle_status: "active".to_string(),
        };
        if behavior.get_embeddable_content(&probe).is_none() {
            tracing::debug!(
                "Root {} is not embeddable (type: {}), skipping embedding queue",
                root_id,
                root_type
            );
            return;
        }

        // Check if embedding exists for this root
        let has_embedding = match store.has_embeddings(&root_id).await {
            Ok(has) => has,
            Err(e) => {
                tracing::warn!(
                    "Failed to check embeddings for root {} (assuming none exist): {}",
                    root_id,
                    e
                );
                false
            }
        };

        // Mark existing embedding as stale or create new stale marker
        let result = if has_embedding {
            store.mark_root_embedding_stale(&root_id).await
        } else {
            store.create_stale_embedding_marker(&root_id).await
        };

        if let Err(e) = result {
            tracing::warn!(
                "Failed to queue root {} for embedding (via node {}): {}",
                root_id,
                node_id,
                e
            );
        } else {
            tracing::debug!(
                "📥 Queued root {} for embedding (triggered by node {})",
                root_id,
                node_id
            );

            // Wake the embedding processor (fire-and-forget)
            if let Some(waker) = embedding_waker {
                tracing::debug!("🔔 Waking embedding processor for root {}", root_id);
                waker.wake();
            }
        }
    }
}
