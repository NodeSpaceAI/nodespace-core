//! Adversarial offline-convergence test for collection-name collisions.
//!
//! Scope: this file covers the OLDER, harder, pre-ADR-065 hard-uniqueness
//! check that used to live in `SqliteStore::create_node` for the `collection`
//! node type specifically — a hardcoded, type-specific check, unrelated to
//! the schema-declared `unique` field mechanism ADR-065 introduced (see
//! `person_duplicate_convergence_test.rs`, which explicitly scopes itself
//! OUT of this check). Before the fix under test, `SqliteStore::create_node`
//! called `bail!` on a collection-name collision; that error propagated
//! straight out of `nodespace-sync`'s `apply_node_upsert`, and per that
//! function's own contract the sync cursor never advances past a failed row
//! — a benign duplicate collection name could wedge a sync cursor
//! permanently.
//!
//! The core invariant under test: NodeSpace is local-first, so collection-name
//! uniqueness can never be *enforced* at creation — two offline devices can
//! each validly create a collection named "Work", and the conflict only
//! becomes visible once both copies land in one database (sync convergence).
//! Hard rejection anywhere in that path would turn an ordinary benign
//! duplicate into a stuck sync. That must never happen.
//!
//! Like its person-duplicate counterpart, this test does not mock the
//! two-device scenario: it stands up fully independent `SqliteStore` +
//! `NodeService` pairs (separate temp directories, no shared state, no
//! coordination) to play the role of independent offline devices, and only
//! performs "convergence" by applying a peer's fully-formed node into another
//! device's store via the real `NodeService::create_node` path
//! `nodespace-sync`'s `apply_node_upsert` also uses.
//!
//! Collections are created here via `Node::new` (a fresh random UUID id) —
//! deliberately NOT via `CollectionService::create_collection` /
//! `collection_ops::create_collection`, which derive a deterministic id from
//! the normalized name specifically so two devices creating the same name
//! converge onto ONE node id instead of ever hitting this collision at all.
//! The vulnerable path this bug affects is any caller that creates a
//! `collection` node directly (an import, an agent tool, or — critically — a
//! sync-pulled peer node, which always keeps the peer's own id) rather than
//! going through that higher-level, id-deterministic helper.

#[cfg(test)]
mod collection_name_convergence_tests {
    use anyhow::Result;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::models::conflict::{ConflictKind, ConflictStatus};
    use nodespace_core::models::{Node, NodeUpdate};
    use nodespace_core::ops::collection_ops::{create_collection, CreateCollectionInput};
    use nodespace_core::ops::OpsError;
    use nodespace_core::services::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// One simulated device: its own on-disk SQLite file, its own `NodeService`,
    /// no relationship to any other `Device` in the test.
    struct Device {
        service: NodeService,
        _temp_dir: TempDir, // kept alive for the duration of the test
    }

    async fn device() -> Result<Device> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("device.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await?);
        let service = NodeService::new(&mut store).await?;
        Ok(Device {
            service,
            _temp_dir: temp_dir,
        })
    }

    /// Apply a fully-formed `Node` (as fetched from a peer device) into `into`'s
    /// store, preserving its id/content/properties exactly as sync-pull would,
    /// via the "node absent locally" branch of a real apply (a fresh
    /// `create_node`).
    async fn apply_incoming(into: &NodeService, incoming: Node) -> Result<String> {
        let id = into.create_node(incoming).await?;
        Ok(id)
    }

    /// Does an OPEN `CollectionNameCollision` conflict record name this node?
    /// Replaces the old `properties.collection._possible_duplicate` boolean
    /// (ADR-068): the journal is the single source of truth, queried the same
    /// way the resolution surface would (`conflicts_for_node`), never
    /// re-derived from a property the write path used to stamp.
    async fn marked(service: &NodeService, node_id: &str) -> Result<bool> {
        let records = service.conflicts_for_node(node_id).await?;
        Ok(records.iter().any(|r| {
            r.kind == ConflictKind::CollectionNameCollision && r.status == ConflictStatus::Open
        }))
    }

    #[tokio::test]
    async fn two_offline_devices_create_the_same_collection_name_and_converge_without_rejection(
    ) -> Result<()> {
        // --- Device A, entirely offline, unaware Device B exists ---
        let device_a = device().await?;
        let work_a_id = device_a
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Work".to_string(),
                json!({}),
            ))
            .await?;

        // --- Device B, entirely offline, unaware Device A exists ---
        // A different collection of the SAME name gets created independently —
        // different device, different local id (a fresh random UUID, exactly
        // as any collection-node create not going through the deterministic-id
        // helper would get — e.g. a sync-pulled peer node), same name.
        let device_b = device().await?;
        let work_b_id = device_b
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Work".to_string(),
                json!({}),
            ))
            .await?;

        assert_ne!(
            work_a_id, work_b_id,
            "the two devices must have produced genuinely distinct node ids"
        );

        // Both devices' local writes succeeded independently — neither device
        // ever saw the other's data, so nothing could have been rejected. This
        // is the local-first baseline the rest of the test builds on.
        assert!(device_a.service.get_node(&work_a_id).await?.is_some());
        assert!(device_b.service.get_node(&work_b_id).await?.is_some());

        // --- Convergence: Device A pulls Device B's node in (sync-apply) ---
        // Fetch B's fully-formed node exactly as a sync pull would receive it
        // over the wire, then apply it into A's store, id and all.
        let bs_node = device_b
            .service
            .get_node(&work_b_id)
            .await?
            .expect("device B's node must exist");

        let applied_id = apply_incoming(&device_a.service, bs_node.clone())
            .await
            .expect(
                "sync apply must NEVER reject a write because of a collection-name \
                 collision — a hard error here is exactly the bug that wedges a sync \
                 cursor permanently on a benign duplicate name",
            );
        assert_eq!(applied_id, work_b_id, "the incoming node keeps its own id");

        // --- No data loss: BOTH nodes now exist, side by side, in A's store ---
        let a_after = device_a
            .service
            .get_node(&work_a_id)
            .await?
            .expect("Device A's own collection must still exist");
        let b_after = device_a
            .service
            .get_node(&work_b_id)
            .await?
            .expect("Device B's synced-in collection must now exist in A's store");
        assert_eq!(a_after.content, "Work");
        assert_eq!(b_after.content, "Work");
        assert_eq!(a_after.node_type, "collection");
        assert_eq!(b_after.node_type, "collection");

        // --- Both sides are journaled as a CollectionNameCollision, automatically ---
        // Unlike the person-email mechanism (detection wired into
        // create_node/update_node's own post-commit hook), the collection-name
        // journal write is triggered synchronously inside
        // `SqliteStore::create_node` itself, so no additional call is needed
        // here — the record must already exist.
        assert!(
            marked(&device_a.service, &work_a_id).await?,
            "Device A's own collection must be journaled"
        );
        assert!(
            marked(&device_a.service, &work_b_id).await?,
            "Device B's synced-in collection must be journaled"
        );

        let a_records = device_a.service.conflicts_for_node(&work_a_id).await?;
        assert_eq!(a_records.len(), 1);
        assert_eq!(a_records[0].kind, ConflictKind::CollectionNameCollision);
        assert_eq!(a_records[0].detail["name"], "work");
        let mut sorted_expected = vec![work_a_id.clone(), work_b_id.clone()];
        sorted_expected.sort();
        assert_eq!(a_records[0].node_ids, sorted_expected);

        Ok(())
    }

    #[tokio::test]
    async fn convergence_with_no_name_collision_marks_nothing_and_still_never_rejects() -> Result<()>
    {
        let device_a = device().await?;
        let work_id = device_a
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Work".to_string(),
                json!({}),
            ))
            .await?;

        let device_b = device().await?;
        let personal_id = device_b
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Personal".to_string(),
                json!({}),
            ))
            .await?;

        let personal_node = device_b.service.get_node(&personal_id).await?.unwrap();
        let applied_id = apply_incoming(&device_a.service, personal_node).await?;
        assert_eq!(applied_id, personal_id);

        // Both distinct collections now coexist in A with no collision at all.
        assert!(device_a.service.get_node(&work_id).await?.is_some());
        assert!(
            !marked(&device_a.service, &personal_id).await?,
            "two genuinely distinct collection names must never be journaled"
        );
        assert!(
            !marked(&device_a.service, &work_id).await?,
            "no record must exist for the pre-existing side absent a real collision"
        );

        Ok(())
    }

    /// `get_collection_by_name` (the predicate both the pre-existing hard check
    /// and this fix's collision detection use) matches case-insensitively. A
    /// collision that differs only in case must be detected and marked exactly
    /// like a byte-identical collision, never rejected.
    #[tokio::test]
    async fn case_insensitive_collection_name_collision_converges_without_rejection() -> Result<()>
    {
        let device_a = device().await?;
        let work_a_id = device_a
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Work".to_string(),
                json!({}),
            ))
            .await?;

        let device_b = device().await?;
        let work_b_id = device_b
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "WORK".to_string(),
                json!({}),
            ))
            .await?;

        let bs_node = device_b.service.get_node(&work_b_id).await?.unwrap();
        let applied_id = apply_incoming(&device_a.service, bs_node)
            .await
            .expect("a case-varied collection-name collision must never be rejected");
        assert_eq!(applied_id, work_b_id);

        let a_after = device_a.service.get_node(&work_a_id).await?.unwrap();
        let b_after = device_a.service.get_node(&work_b_id).await?.unwrap();
        assert_eq!(
            a_after.content, "Work",
            "each side's own casing is preserved"
        );
        assert_eq!(
            b_after.content, "WORK",
            "each side's own casing is preserved"
        );
        assert!(marked(&device_a.service, &work_a_id).await?);
        assert!(marked(&device_a.service, &work_b_id).await?);

        Ok(())
    }

    /// A narrow, real concurrency case: two different peer devices
    /// concurrently push a same-named collection into ONE hub that already
    /// holds a THIRD, pre-existing collection of that name — as two racing
    /// sync-pull tasks might. The hub's own collection is durably committed
    /// BEFORE either concurrent apply starts, so each apply's pre-insert
    /// collision check (`get_collection_by_name`) is guaranteed to see it
    /// regardless of how the two concurrent applies interleave with EACH
    /// OTHER — unlike a race between two brand-new nodes with no pre-existing
    /// third party, where the marking outcome would genuinely depend on
    /// unspecified SELECT/INSERT interleaving (a real, accepted TOCTOU gap;
    /// see `mark_collection_name_collision`'s doc comment). This design lets
    /// the test assert journal records deterministically while still
    /// exercising real concurrent writers, not just proving the no-rejection
    /// guarantee isn't an artifact of strict sequencing.
    ///
    /// Each concurrent apply detects its OWN pairwise collision against the
    /// pre-existing hub node (hub+A, hub+B) — two distinct
    /// `CollectionNameCollision` records, not one record naming all three,
    /// since detection always compares the just-written node against
    /// whatever `get_collection_by_name` finds at that moment (one match).
    #[tokio::test]
    async fn concurrent_convergence_of_two_same_named_collections_never_rejects() -> Result<()> {
        let hub = Arc::new(device().await?);
        let hub_id = hub
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Concurrent".to_string(),
                json!({}),
            ))
            .await?;

        let device_a = device().await?;
        let a_id = device_a
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Concurrent".to_string(),
                json!({}),
            ))
            .await?;
        let a_node = device_a.service.get_node(&a_id).await?.unwrap();

        let device_b = device().await?;
        let b_id = device_b
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Concurrent".to_string(),
                json!({}),
            ))
            .await?;
        let b_node = device_b.service.get_node(&b_id).await?.unwrap();

        let hub_a = Arc::clone(&hub);
        let hub_b = Arc::clone(&hub);
        let (result_a, result_b) = tokio::join!(
            tokio::spawn(async move { apply_incoming(&hub_a.service, a_node).await }),
            tokio::spawn(async move { apply_incoming(&hub_b.service, b_node).await }),
        );

        result_a
            .expect("task must not panic")
            .expect("concurrent apply of A must never be rejected on a name collision");
        result_b
            .expect("task must not panic")
            .expect("concurrent apply of B must never be rejected on a name collision");

        assert!(
            marked(&hub.service, &hub_id).await?,
            "hub's pre-existing collection must be journaled — it was durably committed \
             before the race started, so both concurrent applies' collision checks are \
             guaranteed to see it regardless of interleaving"
        );
        assert!(
            marked(&hub.service, &a_id).await?,
            "A's concurrently-applied collection must be journaled"
        );
        assert!(
            marked(&hub.service, &b_id).await?,
            "B's concurrently-applied collection must be journaled"
        );

        // Two distinct pairwise records (hub+A, hub+B) — the hub node
        // participates in both.
        let hub_records = hub.service.conflicts_for_node(&hub_id).await?;
        assert_eq!(
            hub_records.len(),
            2,
            "the hub's node participates in two distinct pairwise collisions"
        );

        Ok(())
    }

    /// Unlike `person`'s convergence detection (`find_conflicting_unique` is
    /// `LIMIT 1`, so with three mutually-colliding people, some pairwise
    /// collisions can go undetected — see `person_duplicate_convergence_test.rs`'s
    /// three-way test), `get_collection_by_name` is ALSO `LIMIT 1`, but
    /// because collection-name detection runs automatically on EVERY create
    /// (not opt-in), each new arrival's own create-time check against the
    /// then-current state is enough for every one of the three nodes to end
    /// up naming at least one open `CollectionNameCollision` record by the
    /// time all three have converged — even though no single record names
    /// all three at once (each record is a pairwise detection).
    #[tokio::test]
    async fn three_way_name_collision_all_survive_and_all_get_marked() -> Result<()> {
        let hub = device().await?;
        let mut ids = Vec::new();

        for i in 0..3 {
            let d = device().await?;
            let id = d
                .service
                .create_node(Node::new(
                    "collection".to_string(),
                    "Triple".to_string(),
                    json!({}),
                ))
                .await?;
            let node = d.service.get_node(&id).await?.unwrap();

            let applied = apply_incoming(&hub.service, node)
                .await
                .unwrap_or_else(|e| {
                    panic!("sync apply must never reject copy {i} on a name collision: {e}")
                });
            assert_eq!(applied, id);
            ids.push(id);
        }

        // All three survive side by side.
        for id in &ids {
            assert!(hub.service.get_node(id).await?.is_some());
        }

        for (i, id) in ids.iter().enumerate() {
            assert!(
                marked(&hub.service, id).await?,
                "device {i}'s collection must be named by at least one open record"
            );
        }

        Ok(())
    }

    /// Rename-path collision marking (closes the gap the previous version of
    /// this test documented): a sync-applied rename — `apply_node_upsert`'s
    /// "node already exists locally" branch, which calls `NodeService::update_node`
    /// (backed by `SqliteStore::update_node_with_version_check`) directly,
    /// never `create_node` — that introduces a fresh name collision with a
    /// different local collection is now detected and marks BOTH sides, the
    /// same as create_node's collision handling.
    #[tokio::test]
    async fn update_path_introducing_a_name_collision_is_now_marked() -> Result<()> {
        let hub = device().await?;
        let existing_id = hub
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Original".to_string(),
                json!({}),
            ))
            .await?;

        let renamed_id = hub
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "ToRename".to_string(),
                json!({}),
            ))
            .await?;
        let renamed_before = hub.service.get_node(&renamed_id).await?.unwrap();

        // Simulate a sync-applied rename (apply_node_upsert's "node already
        // exists locally" branch, which calls `update_node` directly) that
        // introduces a collision with `existing_id`'s name.
        let updated = hub
            .service
            .update_node(
                &renamed_id,
                renamed_before.version,
                NodeUpdate::new().with_content("Original".to_string()),
            )
            .await
            .expect("a name collision on rename must never be rejected, only marked");
        assert_eq!(updated.content, "Original");

        assert!(
            marked(&hub.service, &existing_id).await? && marked(&hub.service, &renamed_id).await?,
            "a rename introducing a fresh name collision must journal both the renamed node \
             and the pre-existing collection it now collides with"
        );

        Ok(())
    }

    /// Same scenario as above but through the OTHER update entry point:
    /// `SqliteStore::update_node` (the plain, non-version-checked sibling,
    /// reached via `NodeService::update_node_unchecked` — used by, e.g., the
    /// schema-node update path). Both entry points must behave identically.
    #[tokio::test]
    async fn unchecked_update_path_introducing_a_name_collision_is_marked() -> Result<()> {
        let hub = device().await?;
        let existing_id = hub
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Original".to_string(),
                json!({}),
            ))
            .await?;

        let renamed_id = hub
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "ToRename".to_string(),
                json!({}),
            ))
            .await?;

        hub.service
            .update_node_unchecked(
                &renamed_id,
                NodeUpdate::new().with_content("Original".to_string()),
            )
            .await
            .expect("a name collision on an unchecked rename must never be rejected");

        assert!(
            marked(&hub.service, &existing_id).await? && marked(&hub.service, &renamed_id).await?,
            "the unchecked update path must journal both sides of a rename-introduced collision, \
             same as the version-checked path"
        );

        Ok(())
    }

    /// A rename that leaves the name UNCHANGED — including one that changes
    /// only case (folds to the same `LOWER(title)`) — must never self-mark.
    /// `get_collection_by_name`, run before the write, would otherwise match
    /// the node's own pre-write row and produce a spurious "collision"
    /// against itself.
    #[tokio::test]
    async fn case_only_rename_never_self_marks() -> Result<()> {
        let hub = device().await?;
        let id = hub
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Engineering".to_string(),
                json!({}),
            ))
            .await?;
        let before = hub.service.get_node(&id).await?.unwrap();

        let updated = hub
            .service
            .update_node(
                &id,
                before.version,
                NodeUpdate::new().with_content("ENGINEERING".to_string()),
            )
            .await?;
        assert_eq!(updated.content, "ENGINEERING");
        assert!(
            !marked(&hub.service, &id).await?,
            "a case-only rename must not match itself as a collision"
        );

        Ok(())
    }

    /// Same self-exclusion guarantee as `case_only_rename_never_self_marks`,
    /// but through the unchecked update path (`SqliteStore::update_node`).
    /// The self-exclusion filter is implemented independently in each of the
    /// two update functions, not shared, so this closes a gap where a
    /// regression specific to the unchecked variant's filter would otherwise
    /// go uncaught.
    #[tokio::test]
    async fn case_only_rename_never_self_marks_via_unchecked_path() -> Result<()> {
        let hub = device().await?;
        let id = hub
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Engineering".to_string(),
                json!({}),
            ))
            .await?;

        hub.service
            .update_node_unchecked(
                &id,
                NodeUpdate::new().with_content("ENGINEERING".to_string()),
            )
            .await?;

        let updated = hub.service.get_node(&id).await?.unwrap();
        assert_eq!(updated.content, "ENGINEERING");
        assert!(
            !marked(&hub.service, &id).await?,
            "a case-only rename must not match itself as a collision, on the unchecked path either"
        );

        Ok(())
    }

    /// Gap 2: `get_collection_by_name` filters to `lifecycle_status =
    /// 'active'`, so archiving a collection genuinely frees up its name — a
    /// new collection (or a rename) reusing that name does not get flagged
    /// `_possible_duplicate` against the archived one, on either the create
    /// path or the rename path.
    #[tokio::test]
    async fn archived_collection_does_not_block_or_mark_name_reuse() -> Result<()> {
        let hub = device().await?;
        let archived_id = hub
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Legacy".to_string(),
                json!({}),
            ))
            .await?;
        let archived_before = hub.service.get_node(&archived_id).await?.unwrap();
        hub.service
            .update_node(
                &archived_id,
                archived_before.version,
                NodeUpdate::new().with_lifecycle_status("archived".to_string()),
            )
            .await?;

        // CREATE path: a fresh collection reusing the archived name.
        let new_id = hub
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Legacy".to_string(),
                json!({}),
            ))
            .await?;
        assert!(
            !marked(&hub.service, &new_id).await? && !marked(&hub.service, &archived_id).await?,
            "creating a collection with an archived collection's name must not journal either \
             side — the archived collection no longer holds that name"
        );

        // RENAME path: a different active collection renamed onto the
        // archived name.
        let other_id = hub
            .service
            .create_node(Node::new(
                "collection".to_string(),
                "Other".to_string(),
                json!({}),
            ))
            .await?;
        let other_before = hub.service.get_node(&other_id).await?.unwrap();
        hub.service
            .update_node(
                &other_id,
                other_before.version,
                NodeUpdate::new().with_content("Legacy".to_string()),
            )
            .await?;
        // `other_id` collides with the ALREADY-ACTIVE `new_id` (created above,
        // in the CREATE-path section) — a real, expected collision, journaled
        // correctly. The archived collection specifically must stay out of
        // it: renaming onto its name must not journal against the archived
        // node, only (if at all) against another currently-active holder of
        // that name.
        assert!(
            !marked(&hub.service, &archived_id).await?,
            "renaming onto an archived collection's name must never journal against the \
             archived node itself"
        );

        Ok(())
    }

    /// Regression for a fix required by gap 2: `collection_ops::create_collection`
    /// derives its new node's id deterministically from the (normalized) name
    /// alone (`deterministic_collection_id`), independent of lifecycle_status.
    /// Its own `AlreadyExists` guard now checks by id rather than by
    /// `get_collection_by_name` (which — correctly, per gap 2 — only matches
    /// ACTIVE collections) precisely so recreating a collection with an
    /// archived collection's name still rejects cleanly, instead of the
    /// active-only name check letting it through into an INSERT that fails
    /// with an opaque primary-key-constraint error (there is no get-or-create
    /// here — `NodeService::create_node` does a plain insert for the
    /// `collection` node type).
    #[tokio::test]
    async fn create_collection_op_rejects_reusing_an_archived_collections_name() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("device.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await?);
        let service = Arc::new(NodeService::new(&mut store).await?);

        let created = create_collection(
            &service,
            CreateCollectionInput {
                name: "Legacy".to_string(),
                description: String::new(),
            },
        )
        .await?;
        let before = service.get_node(&created.collection_id).await?.unwrap();
        service
            .update_node(
                &created.collection_id,
                before.version,
                NodeUpdate::new().with_lifecycle_status("archived".to_string()),
            )
            .await?;

        let result = create_collection(
            &service,
            CreateCollectionInput {
                name: "Legacy".to_string(),
                description: String::new(),
            },
        )
        .await;

        match result {
            Err(OpsError::AlreadyExists { .. }) => {}
            other => panic!(
                "expected OpsError::AlreadyExists when recreating an archived collection's \
                 name, got {other:?}"
            ),
        }

        Ok(())
    }
}
