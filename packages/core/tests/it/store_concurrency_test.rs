//! Concurrency contract for `SqliteStore`.
//!
//! The daemon runs one store per database and hands the same `Arc<SqliteStore>`
//! to every concurrent task (gRPC handlers, import, the embedding processor, the
//! local agent). These tests drive that shape through the public API: many tasks,
//! one store, overlapping transactional writes and reads, asserting that nothing
//! errors and nothing is lost or half-applied.

#[cfg(test)]
mod store_concurrency_tests {
    use anyhow::Result;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::models::{Node, NodeUpdate};
    use nodespace_core::services::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn create_test_store() -> Result<(Arc<SqliteStore>, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await?);
        let _service = NodeService::new(&mut store).await?;
        Ok((store, temp_dir))
    }

    async fn seed_node(store: &SqliteStore, content: &str) -> Result<String> {
        let node = Node::new("text".to_string(), content.to_string(), json!({}));
        let id = node.id.clone();
        store.create_node(node, None, None).await?;
        Ok(id)
    }

    /// Many tasks, each opening its own transaction on the shared store, must all
    /// succeed. With a single shared connection the second overlapping `BEGIN`
    /// fails outright with "cannot start a transaction within a transaction", so
    /// a user action (a subtree delete, a collection add) errors purely because
    /// a background task happened to be committing at the time.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn overlapping_transactional_writers_all_succeed() -> Result<()> {
        let (store, _t) = create_test_store().await?;

        const TASKS: usize = 12;

        // Pre-seed the rows each task will operate on, so the concurrent phase is
        // pure contention rather than setup.
        let mut collection_ids = Vec::new();
        let mut member_ids = Vec::new();
        let mut marker_ids = Vec::new();
        let mut update_ids = Vec::new();
        for i in 0..TASKS {
            let coll = Node::new("collection".to_string(), format!("coll-{i}"), json!({}));
            let coll_id = coll.id.clone();
            store.create_node(coll, None, None).await?;
            collection_ids.push(coll_id);
            member_ids.push(seed_node(&store, &format!("member-{i}")).await?);
            marker_ids.push(seed_node(&store, &format!("marker-{i}")).await?);
            update_ids.push(seed_node(&store, &format!("update-{i}")).await?);
        }

        let mut handles = Vec::new();
        for i in 0..TASKS {
            // add_to_collection: opens a transaction.
            handles.push(tokio::spawn({
                let store = store.clone();
                let member = member_ids[i].clone();
                let coll = collection_ids[i].clone();
                async move {
                    store
                        .add_to_collection(&member, &coll, &json!({}))
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("add_to_collection: {e}"))
                }
            }));
            // create_stale_embedding_markers_bulk: opens a transaction.
            handles.push(tokio::spawn({
                let store = store.clone();
                let marker = marker_ids[i].clone();
                async move {
                    store
                        .create_stale_embedding_markers_bulk(&[marker])
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("create_stale_embedding_markers_bulk: {e}"))
                }
            }));
            // bulk_update: opens a transaction.
            handles.push(tokio::spawn({
                let store = store.clone();
                let id = update_ids[i].clone();
                async move {
                    store
                        .bulk_update(vec![(
                            id,
                            NodeUpdate {
                                content: Some(format!("bulk-{i}")),
                                ..Default::default()
                            },
                        )])
                        .await
                        .map_err(|e| format!("bulk_update: {e}"))
                }
            }));
            // A plain single-statement write racing all of the above.
            handles.push(tokio::spawn({
                let store = store.clone();
                async move {
                    let node = Node::new("text".to_string(), format!("plain-{i}"), json!({}));
                    store
                        .create_node(node, None, None)
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("create_node: {e}"))
                }
            }));
        }

        let mut failures = Vec::new();
        for h in handles {
            if let Err(e) = h.await.expect("task panicked") {
                failures.push(e);
            }
        }
        assert!(
            failures.is_empty(),
            "concurrent transactional writers failed: {failures:#?}"
        );

        // Every write actually landed.
        for i in 0..TASKS {
            assert_eq!(
                store
                    .get_collection_members(&collection_ids[i])
                    .await?
                    .len(),
                1,
                "membership {i} missing"
            );
            assert!(
                store.has_embeddings(&marker_ids[i]).await?,
                "stale marker {i} missing"
            );
            assert_eq!(
                store.get_node(&update_ids[i]).await?.expect("node").content,
                format!("bulk-{i}"),
                "bulk update {i} missing"
            );
        }

        Ok(())
    }

    /// A bulk import that fails and rolls back must roll back ONLY its own work.
    /// Edits made concurrently (the autosave case: the user keeps typing while an
    /// import runs) are acknowledged to their caller and must survive.
    ///
    /// On a shared connection those edits execute inside the import's open
    /// transaction and are destroyed by its `ROLLBACK`, while the caller — and
    /// the frontend behind it — has already been told the write succeeded.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_failed_import_does_not_roll_back_concurrent_edits() -> Result<()> {
        let (store, _t) = create_test_store().await?;

        const EDITS: usize = 8;
        let mut edit_ids = Vec::new();
        for i in 0..EDITS {
            edit_ids.push(seed_node(&store, &format!("before-{i}")).await?);
        }

        // A large hierarchy import whose LAST row duplicates the first id, so the
        // transaction fails on a primary-key conflict after a lot of awaited work.
        let mut rows: Vec<nodespace_core::db::BulkNodeRow> = Vec::new();
        let root_id = uuid::Uuid::new_v4().to_string();
        rows.push((
            root_id.clone(),
            "text".to_string(),
            "import-root".to_string(),
            None,
            1.0,
            json!({}),
            None,
        ));
        for i in 0..400 {
            rows.push((
                uuid::Uuid::new_v4().to_string(),
                "text".to_string(),
                format!("import-{i}"),
                Some(root_id.clone()),
                i as f64,
                json!({}),
                None,
            ));
        }
        // Duplicate id → the whole transaction rolls back.
        rows.push((
            root_id.clone(),
            "text".to_string(),
            "duplicate".to_string(),
            None,
            1.0,
            json!({}),
            None,
        ));

        let importer = tokio::spawn({
            let store = store.clone();
            async move { store.bulk_create_hierarchy(rows).await }
        });

        let mut editors = Vec::new();
        for (i, id) in edit_ids.iter().enumerate() {
            editors.push(tokio::spawn({
                let store = store.clone();
                let id = id.clone();
                async move {
                    store
                        .update_node(
                            &id,
                            NodeUpdate {
                                content: Some(format!("after-{i}")),
                                ..Default::default()
                            },
                            None,
                        )
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("{e}"))
                }
            }));
        }

        assert!(
            importer.await.expect("import task panicked").is_err(),
            "the import was expected to fail on its duplicate id"
        );
        for (i, h) in editors.into_iter().enumerate() {
            h.await
                .expect("editor task panicked")
                .unwrap_or_else(|e| panic!("edit {i} failed: {e}"));
        }

        // The import left nothing behind…
        assert!(
            store.get_node(&root_id).await?.is_none(),
            "the failed import's rows must not be committed"
        );
        // …and took none of the acknowledged edits with it.
        for (i, id) in edit_ids.iter().enumerate() {
            assert_eq!(
                store.get_node(id).await?.expect("edited node").content,
                format!("after-{i}"),
                "edit {i} was rolled back with the unrelated import"
            );
        }

        Ok(())
    }

    /// Reads running against a store that is busy writing must only ever observe
    /// committed state — never a row from a transaction still in flight, and
    /// never a row from one that later rolls back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reads_never_observe_a_rolled_back_import() -> Result<()> {
        let (store, _t) = create_test_store().await?;

        // Ids the import will insert and then roll back. A reader polls for them
        // throughout; none may ever be visible.
        let doomed: Vec<String> = (0..200).map(|_| uuid::Uuid::new_v4().to_string()).collect();

        let mut rows: Vec<nodespace_core::db::BulkNodeRow> = doomed
            .iter()
            .enumerate()
            .map(|(i, id)| {
                (
                    id.clone(),
                    "text".to_string(),
                    format!("doomed-{i}"),
                    None,
                    i as f64,
                    json!({}),
                    None,
                )
            })
            .collect();
        // Duplicate the first id at the end so the transaction rolls back.
        rows.push((
            doomed[0].clone(),
            "text".to_string(),
            "duplicate".to_string(),
            None,
            0.0,
            json!({}),
            None,
        ));

        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let reader = tokio::spawn({
            let store = store.clone();
            let doomed = doomed.clone();
            let done = done.clone();
            async move {
                let mut observed: Option<String> = None;
                let mut polls = 0u64;
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    for id in &doomed {
                        if store.get_node(id).await.expect("read failed").is_some() {
                            observed = Some(id.clone());
                        }
                    }
                    polls += 1;
                    tokio::task::yield_now().await;
                }
                (observed, polls)
            }
        });

        let import = store.bulk_create_hierarchy(rows).await;
        done.store(true, std::sync::atomic::Ordering::Relaxed);

        assert!(
            import.is_err(),
            "the import was expected to fail on its duplicate id"
        );

        let (observed, polls) = reader.await.expect("reader task panicked");
        assert!(polls > 0, "reader never ran");
        assert!(
            observed.is_none(),
            "a read observed node {:?} from an uncommitted (and later rolled back) transaction",
            observed
        );

        // And they are still absent afterwards.
        for id in &doomed {
            assert!(store.get_node(id).await?.is_none());
        }

        Ok(())
    }

    /// Reads must not be serialized behind writes: a reader has to make progress
    /// while a long import holds the write path. This is the reason reads use
    /// their own connection rather than sharing the writer's lock.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reads_make_progress_while_a_long_import_runs() -> Result<()> {
        let (store, _t) = create_test_store().await?;
        let probe = seed_node(&store, "probe").await?;

        let rows: Vec<nodespace_core::db::BulkNodeRow> = (0..2000)
            .map(|i| {
                (
                    uuid::Uuid::new_v4().to_string(),
                    "text".to_string(),
                    format!("row-{i}"),
                    None,
                    i as f64,
                    json!({}),
                    None,
                )
            })
            .collect();

        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = tokio::spawn({
            let store = store.clone();
            let probe = probe.clone();
            let done = done.clone();
            async move {
                let mut reads = 0u64;
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    assert!(store.get_node(&probe).await.expect("read failed").is_some());
                    reads += 1;
                    tokio::task::yield_now().await;
                }
                reads
            }
        });

        store.bulk_create_hierarchy(rows).await?;
        done.store(true, std::sync::atomic::Ordering::Relaxed);

        let reads = reader.await.expect("reader task panicked");
        assert!(
            reads > 1,
            "reader completed only {reads} read(s) during the import — reads are queueing behind writes"
        );

        Ok(())
    }
}
