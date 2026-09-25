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
    /// non-embeddable root such as a `date` page or a `task`. The one
    /// exception is a descendant whose access differs from its root's
    /// (ADR-059 §7): it is cut out of the root's vector and is its own
    /// embedding root. See [`crate::db::SqliteStore::embedding_root_id`].
    pub async fn get_embedding_root_id(&self, node_id: &str) -> Result<String, NodeServiceError> {
        self.store
            .embedding_root_id(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Queue the embedding root of the tree `node_id` just left.
    /// `former_parent` was its `has_child` parent before the write. That
    /// parent's embedding root is the root whose aggregate held the node's
    /// subtree. It is skipped when it is also the node's current root (a move
    /// within one tree), which the caller has already queued.
    #[cfg(feature = "nlp")]
    pub(crate) async fn queue_former_embedding_root(&self, node_id: &str, former_parent: &str) {
        let roots = tokio::try_join!(
            self.get_embedding_root_id(former_parent),
            self.get_embedding_root_id(node_id)
        );
        match roots {
            Ok((former_root, current_root)) if former_root != current_root => {
                self.queue_root_for_embedding(&former_root).await;
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(
                node_id = %node_id,
                former_parent = %former_parent,
                error = %e,
                "failed to resolve the former embedding root (not queued)"
            ),
        }
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
        // Find the embedding root of this node.
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
        // Find the embedding root, as in `NodeService::get_embedding_root_id`.
        let root_id = match store.embedding_root_id(node_id).await {
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

/// A node leaving a tree re-queues the tree it left, not only the one it
/// joined. Each test gives the roots a fresh (non-stale) embedding first, so a
/// stale marker afterwards can only come from the edge change under test.
#[cfg(all(test, feature = "nlp"))]
mod former_embedding_root_tests {
    use crate::db::SqliteStore;
    use crate::models::NewEmbedding;
    use crate::services::error::NodeServiceError;
    use crate::services::{CreateNodeParams, InsertPosition, InsertPositionOwned, NodeService};
    use std::sync::Arc;
    use tempfile::TempDir;

    const ROOT_A: &str = "22222222-0000-0000-0000-00000000000a";
    const ROOT_B: &str = "22222222-0000-0000-0000-00000000000b";
    const LINE: &str = "22222222-0000-0000-0000-0000000000c1";

    async fn service() -> (NodeService, TempDir) {
        let tmp = TempDir::new().unwrap();
        let mut store = Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
        let svc = NodeService::new(&mut store).await.unwrap();
        (svc, tmp)
    }

    async fn text(svc: &NodeService, id: &str, content: &str, parent: Option<&str>) {
        svc.create_node_with_parent(CreateNodeParams {
            id: Some(id.into()),
            node_type: "text".into(),
            content: content.into(),
            parent_id: parent.map(Into::into),
            position: InsertPositionOwned::End,
            properties: serde_json::json!({}),
            lifecycle_status: None,
        })
        .await
        .unwrap();
    }

    /// ROOT_A holds LINE; ROOT_B is a separate tree. Both roots start fresh.
    async fn two_trees() -> (NodeService, TempDir) {
        let (svc, tmp) = service().await;
        text(&svc, ROOT_A, "Root A", None).await;
        text(&svc, LINE, "LINE_TEXT", Some(ROOT_A)).await;
        text(&svc, ROOT_B, "Root B", None).await;
        embed_fresh(&svc, &[ROOT_A, ROOT_B]).await;
        (svc, tmp)
    }

    async fn embed_fresh(svc: &NodeService, ids: &[&str]) {
        for id in ids {
            svc.upsert_embeddings(
                id,
                vec![NewEmbedding::single_chunk(*id, vec![0.5; 768], "h", 1, 1)],
            )
            .await
            .unwrap();
            assert!(!is_stale(svc, id).await, "{id} must start fresh");
        }
    }

    async fn is_stale(svc: &NodeService, id: &str) -> bool {
        svc.get_embeddings(id)
            .await
            .unwrap()
            .iter()
            .any(|e| e.stale)
    }

    async fn version(svc: &NodeService, id: &str) -> i64 {
        svc.get_node(id).await.unwrap().unwrap().version
    }

    #[tokio::test]
    async fn outdent_to_root_requeues_the_former_root() {
        let (svc, _tmp) = two_trees().await;

        svc.move_node(LINE, version(&svc, LINE).await, None, InsertPosition::End)
            .await
            .unwrap();

        assert!(is_stale(&svc, ROOT_A).await, "the tree LINE left");
        assert!(is_stale(&svc, LINE).await, "LINE, now a root");
        assert!(!is_stale(&svc, ROOT_B).await, "an uninvolved tree");
    }

    #[tokio::test]
    async fn unchecked_outdent_requeues_the_former_root() {
        let (svc, _tmp) = two_trees().await;

        svc.move_node_unchecked(LINE, None, InsertPosition::End)
            .await
            .unwrap();

        assert!(is_stale(&svc, ROOT_A).await, "the tree LINE left");
    }

    #[tokio::test]
    async fn reparent_across_trees_requeues_both_roots() {
        let (svc, _tmp) = two_trees().await;

        svc.move_node(
            LINE,
            version(&svc, LINE).await,
            Some(ROOT_B),
            InsertPosition::End,
        )
        .await
        .unwrap();

        assert!(is_stale(&svc, ROOT_A).await, "the tree LINE left");
        assert!(is_stale(&svc, ROOT_B).await, "the tree LINE joined");
    }

    #[tokio::test]
    async fn create_parent_edge_reparent_requeues_the_former_root() {
        let (svc, _tmp) = two_trees().await;

        svc.create_parent_edge(LINE, ROOT_B, InsertPosition::End)
            .await
            .unwrap();

        assert!(is_stale(&svc, ROOT_A).await, "the tree LINE left");
        assert!(is_stale(&svc, ROOT_B).await, "the tree LINE joined");
    }

    #[tokio::test]
    async fn deleting_the_has_child_edge_requeues_the_former_root() {
        let (svc, _tmp) = two_trees().await;

        svc.delete_relationship(ROOT_A, "has_child", LINE)
            .await
            .unwrap();

        assert!(is_stale(&svc, ROOT_A).await, "the tree LINE left");
    }

    #[tokio::test]
    async fn tx_edge_delete_requeues_the_former_root_after_commit() {
        let (svc, _tmp) = two_trees().await;

        let inner = svc.clone();
        svc.with_transaction(move |tx| {
            Box::pin(async move {
                inner
                    .remove_relationship_in_tx(tx, ROOT_A, "has_child", LINE)
                    .await
            })
        })
        .await
        .unwrap();

        assert!(is_stale(&svc, ROOT_A).await, "the tree LINE left");
    }

    #[tokio::test]
    async fn rolled_back_tx_edge_delete_queues_nothing() {
        let (svc, _tmp) = two_trees().await;

        let inner = svc.clone();
        let result: Result<(), NodeServiceError> = svc
            .with_transaction(move |tx| {
                Box::pin(async move {
                    inner
                        .remove_relationship_in_tx(tx, ROOT_A, "has_child", LINE)
                        .await?;
                    Err(NodeServiceError::invalid_update("roll back"))
                })
            })
            .await;

        assert!(result.is_err());
        assert!(!is_stale(&svc, ROOT_A).await, "nothing committed");
        assert!(svc.get_embeddings(LINE).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_move_within_one_tree_queues_only_that_tree() {
        const SIBLING: &str = "22222222-0000-0000-0000-0000000000c2";
        let (svc, _tmp) = two_trees().await;
        text(&svc, SIBLING, "Sibling", Some(ROOT_A)).await;
        embed_fresh(&svc, &[ROOT_A]).await;

        svc.move_node(
            LINE,
            version(&svc, LINE).await,
            Some(SIBLING),
            InsertPosition::End,
        )
        .await
        .unwrap();

        assert!(is_stale(&svc, ROOT_A).await, "the tree LINE moved within");
        assert!(!is_stale(&svc, ROOT_B).await, "an uninvolved tree");
        for id in [LINE, SIBLING] {
            assert!(
                svc.get_embeddings(id).await.unwrap().is_empty(),
                "{id} is a child and is never queued"
            );
        }
    }

    /// ADR-059 §7 through a legitimate flow: outdenting a line makes it a root,
    /// which may then be filed into a restricted collection. The open tree it
    /// left must be re-embedded, or its vector keeps the line's meaning.
    #[tokio::test]
    async fn outdent_then_file_into_restricted_collection_requeues_the_open_root() {
        use crate::behaviors::{NodeBehavior, TextNodeBehavior};
        const RESTRICTED: &str = "22222222-0000-0000-0000-0000000000d1";

        let (svc, _tmp) = two_trees().await;
        svc.create_node_with_parent(CreateNodeParams {
            id: Some(RESTRICTED.into()),
            node_type: "collection".into(),
            content: "Restricted".into(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties: serde_json::json!({ "collection": { "restrictedToMembers": true } }),
            lifecycle_status: None,
        })
        .await
        .unwrap();

        svc.move_node(LINE, version(&svc, LINE).await, None, InsertPosition::End)
            .await
            .unwrap();
        svc.store()
            .add_to_collection(LINE, RESTRICTED, &serde_json::json!({}))
            .await
            .expect("an outdented line is a root and may be filed");

        assert!(is_stale(&svc, ROOT_A).await, "the open tree LINE left");
        let root_a = svc.get_node(ROOT_A).await.unwrap().unwrap();
        let rebuilt = TextNodeBehavior
            .get_aggregated_content(&root_a, &svc)
            .await
            .unwrap_or_default();
        assert!(
            !rebuilt.contains("LINE_TEXT"),
            "the rebuilt aggregate excludes the filed line: {rebuilt}"
        );
    }
}
