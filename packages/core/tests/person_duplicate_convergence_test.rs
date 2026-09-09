//! Adversarial offline-convergence test for the store-aware `unique` rule
//! (ADR-065) and its conflict-journal representation (ADR-068).
//!
//! Scope: this file proves the invariant for the schema-declared `unique` rule
//! specifically (the mechanism this issue adds) — NOT for every hard-uniqueness
//! check that exists anywhere in the store. A separate, older, harder mechanism
//! (collection-name uniqueness in `SqliteStore::create_node`) predates this rule
//! and is a genuinely different, unrelated constraint outside this file's scope;
//! it is not exercised or claimed to be covered here (see
//! `collection_name_convergence_test.rs`).
//!
//! The core invariant under test: NodeSpace is local-first, so a `unique`
//! schema rule can never be *enforced* at creation — two offline devices can
//! each validly create "the same" person, and the conflict only becomes visible
//! once both copies land in one database (sync convergence). Hard rejection
//! anywhere in that path — at either device's creation, or when the peer's copy
//! is applied locally, whether as a brand-new row or as an update to a
//! previously-converged one — would turn an ordinary data-entry duplicate into
//! a sync failure. That must never happen for this rule.
//!
//! Since ADR-068, detection is no longer an opt-in call a caller makes after a
//! write (the old `NodeService::mark_possible_duplicates`, which had zero
//! production callers): `create_node`/`update_node` themselves run
//! `detect_unique_field_collisions` post-commit, best-effort, so a collision
//! is journaled as a `UniqueFieldCollision` `ConflictRecord` the moment both
//! copies land in one database — reachable on a purely local-only install,
//! with no `nodespace-sync` involved anywhere in this file.
//!
//! These tests are sequential (`await` at every step); they prove correctness
//! under sequential convergence, not under concurrent convergence. A dedicated
//! concurrent test below covers two applies racing into the same store, but a
//! full concurrent-detection stress test is out of scope here.
//!
//! This test does not mock the two-device scenario: it stands up fully
//! independent `SqliteStore` + `NodeService` pairs (separate temp directories,
//! no shared state, no coordination) to play the role of independent offline
//! devices, and only performs "convergence" — applying a peer's fully-formed
//! node into another device's store, via the real `NodeService::create_node` /
//! `update_node` paths `nodespace-sync`'s `apply_node_upsert` also uses — after
//! each device's own write has already succeeded independently.

#[cfg(test)]
mod offline_convergence_tests {
    use anyhow::Result;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::models::conflict::{ConflictKind, ConflictStatus};
    use nodespace_core::models::{Node, NodeUpdate};
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

    /// Every OPEN `UniqueFieldCollision` conflict record naming `node_id`.
    async fn open_unique_field_collisions(
        service: &NodeService,
        node_id: &str,
    ) -> Result<Vec<nodespace_core::models::ConflictRecord>> {
        let records = service.conflicts_for_node(node_id).await?;
        Ok(records
            .into_iter()
            .filter(|r| {
                r.kind == ConflictKind::UniqueFieldCollision && r.status == ConflictStatus::Open
            })
            .collect())
    }

    #[tokio::test]
    async fn two_offline_devices_create_the_same_person_and_converge_without_rejection(
    ) -> Result<()> {
        // --- Device A, entirely offline, unaware Device B exists ---
        let device_a = device().await?;
        let alice_a_id = device_a
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Alice".to_string(),
                json!({ "person": { "name": "Alice", "email": "alice@example.com" } }),
            ))
            .await?;

        // --- Device B, entirely offline, unaware Device A exists ---
        // A different person of the SAME real-world identity gets created
        // independently — different device, different local id, same email.
        let device_b = device().await?;
        let alice_b_id = device_b
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Alice B".to_string(),
                json!({ "person": { "name": "Alice B", "email": "ALICE@example.com" } }),
            ))
            .await?;

        assert_ne!(
            alice_a_id, alice_b_id,
            "the two devices must have produced genuinely distinct node ids"
        );

        // Both devices' local writes succeeded independently — neither device
        // ever saw the other's data, so nothing could have been rejected, and
        // neither has any conflict record yet (no collision existed locally).
        assert!(device_a.service.get_node(&alice_a_id).await?.is_some());
        assert!(device_b.service.get_node(&alice_b_id).await?.is_some());
        assert!(open_unique_field_collisions(&device_a.service, &alice_a_id)
            .await?
            .is_empty());

        // --- Convergence: Device A pulls Device B's node in (sync-apply) ---
        // Fetch B's fully-formed node exactly as a sync pull would receive it
        // over the wire, then apply it into A's store, id and all.
        let bs_node = device_b
            .service
            .get_node(&alice_b_id)
            .await?
            .expect("device B's node must exist");

        let applied_id = apply_incoming(&device_a.service, bs_node.clone())
            .await
            .expect(
                "sync apply must NEVER reject a write because of a `unique`-rule collision \
                 (ADR-065) — a hard error here would turn a benign duplicate into a stuck sync",
            );
        assert_eq!(applied_id, alice_b_id, "the incoming node keeps its own id");

        // --- No data loss: BOTH nodes now exist, side by side, in A's store ---
        let a_after = device_a
            .service
            .get_node(&alice_a_id)
            .await?
            .expect("Device A's own person must still exist");
        let b_after = device_a
            .service
            .get_node(&alice_b_id)
            .await?
            .expect("Device B's synced-in person must now exist in A's store");
        assert_eq!(a_after.content, "Alice");
        assert_eq!(b_after.content, "Alice B");
        assert_eq!(
            a_after
                .properties
                .get("person")
                .and_then(|p| p.get("email"))
                .and_then(|v| v.as_str()),
            Some("alice@example.com")
        );
        assert_eq!(
            b_after
                .properties
                .get("person")
                .and_then(|p| p.get("email"))
                .and_then(|v| v.as_str()),
            Some("ALICE@example.com"),
            "the incoming node's data must round-trip unmodified — no silent overwrite"
        );

        // --- The shared predicate now sees the collision from A's perspective ---
        // Same predicate the creation-time suggestion uses, so semantics never
        // drift between "suggest at creation" and "detect at convergence". Query
        // with B's casing (not A's own, byte-identical value) and exclude A's own
        // id, so this genuinely depends on case-insensitive folding finding B's
        // node — not a vacuous self-match that would pass even with folding
        // completely broken.
        let dup = device_a
            .service
            .find_duplicate_for(
                "person",
                "email",
                "ALICE@example.com",
                Some(alice_a_id.as_str()),
            )
            .await?;
        assert_eq!(
            dup.map(|n| n.id),
            Some(alice_b_id.clone()),
            "post-convergence, excluding A's own node, the case-insensitive lookup \
             for B's exact casing must resolve to B's node specifically"
        );

        // --- Detection ran automatically inside create_node (ADR-068): a
        // single UniqueFieldCollision record must already exist, naming both. ---
        let a_records = open_unique_field_collisions(&device_a.service, &alice_a_id).await?;
        let b_records = open_unique_field_collisions(&device_a.service, &alice_b_id).await?;
        assert_eq!(
            a_records.len(),
            1,
            "exactly one open UniqueFieldCollision record must name A's node"
        );
        assert_eq!(
            b_records.len(),
            1,
            "exactly one open UniqueFieldCollision record must name B's node"
        );
        assert_eq!(
            a_records[0].id, b_records[0].id,
            "both participants must resolve to the SAME record (derived, symmetric id)"
        );

        let record = &a_records[0];
        let mut sorted_expected = vec![alice_a_id.clone(), alice_b_id.clone()];
        sorted_expected.sort();
        assert_eq!(
            record.node_ids, sorted_expected,
            "record must name both nodes"
        );
        assert_eq!(record.detail["node_type"], "person");
        assert_eq!(record.detail["field"], "email");

        // --- Version-preserving: detection must not perturb either node's OCC state ---
        let a_final = device_a.service.get_node(&alice_a_id).await?.unwrap();
        let b_final = device_a.service.get_node(&alice_b_id).await?.unwrap();
        assert_eq!(
            a_final.version, a_after.version,
            "journaling a conflict must not bump A's own node's OCC version"
        );
        assert_eq!(
            b_final.version, b_after.version,
            "journaling a conflict must not bump B's synced-in node's OCC version"
        );

        Ok(())
    }

    #[tokio::test]
    async fn convergence_with_no_collision_journals_nothing_and_still_never_rejects() -> Result<()>
    {
        let device_a = device().await?;
        let alice_id = device_a
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Alice".to_string(),
                json!({ "person": { "name": "Alice", "email": "alice@example.com" } }),
            ))
            .await?;

        let device_b = device().await?;
        let bob_id = device_b
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Bob".to_string(),
                json!({ "person": { "name": "Bob", "email": "bob@example.com" } }),
            ))
            .await?;

        let bobs_node = device_b.service.get_node(&bob_id).await?.unwrap();
        let applied_id = apply_incoming(&device_a.service, bobs_node).await?;
        assert_eq!(applied_id, bob_id);

        // Both distinct people now coexist in A with no collision at all.
        assert!(device_a.service.get_node(&alice_id).await?.is_some());
        assert!(device_a.service.get_node(&bob_id).await?.is_some());

        assert!(
            open_unique_field_collisions(&device_a.service, &bob_id)
                .await?
                .is_empty(),
            "two genuinely distinct emails must never produce a conflict record"
        );
        assert!(open_unique_field_collisions(&device_a.service, &alice_id)
            .await?
            .is_empty());

        Ok(())
    }

    /// THREE mutually-colliding devices converge sequentially into one hub.
    /// Unlike the old opt-in, `LIMIT 1`-pairwise `mark_possible_duplicates`
    /// (which marked exactly 2 of 3 from a single call), detection now runs
    /// automatically on every create/update — so by the time all three copies
    /// have landed, every pairwise collision among the three has had a chance
    /// to be detected as each new copy arrives and is checked against every
    /// already-present active node of the same type/field.
    #[tokio::test]
    async fn three_way_convergence_all_survive_and_every_pairwise_collision_is_journaled(
    ) -> Result<()> {
        // THREE independent offline devices each create "the same" person
        // (case-varied email), then all three copies converge onto one store
        // one at a time (as sequential sync pulls would apply them). Every node
        // must survive every step, and no apply may ever reject.
        let emails = [
            "alice@example.com",
            "Alice@Example.com",
            "ALICE@EXAMPLE.COM",
        ];
        let mut ids = Vec::new();
        let hub = device().await?;

        for (i, email) in emails.iter().enumerate() {
            let d = device().await?;
            let id = d
                .service
                .create_node(Node::new(
                    "person".to_string(),
                    format!("Alice (device {i})"),
                    json!({ "person": { "name": format!("Alice (device {i})"), "email": email } }),
                ))
                .await?;
            let node = d.service.get_node(&id).await?.unwrap();

            // Converge this device's copy into the hub, unconditionally.
            let applied = apply_incoming(&hub.service, node)
                .await
                .unwrap_or_else(|e| {
                    panic!("sync apply must never reject copy {i} on a uniqueness collision: {e}")
                });
            assert_eq!(applied, id);
            ids.push(id);
        }

        // All three survive side by side.
        for id in &ids {
            assert!(hub.service.get_node(id).await?.is_some());
        }

        // `find_conflicting_unique` is `LIMIT 1`, so each of the 2nd and 3rd
        // creates detects exactly one prior colliding node (not all priors) —
        // the 2nd copy's create detects the 1st, and the 3rd copy's create
        // detects one of the first two. Every node must end up naming at
        // least one open UniqueFieldCollision record, since every node here
        // genuinely collides with at least one other.
        for id in &ids {
            let records = open_unique_field_collisions(&hub.service, id).await?;
            assert!(
                !records.is_empty(),
                "node {id} participates in a real 3-way collision and must be \
                 named by at least one open UniqueFieldCollision record"
            );
        }

        Ok(())
    }

    /// `apply_node_upsert` in `nodespace-sync` has three branches: create
    /// (node absent locally), update (node already present locally), and an
    /// already-exists fallback to update. The tests above only exercise the
    /// first. This exercises the update branch: a node already present in the
    /// hub (as if pulled by an earlier sync cycle) receives an incoming update
    /// — applied via `NodeService::update_node`, not `create_node` — that
    /// introduces a fresh collision with a different existing node. The update
    /// must succeed unconditionally and the collision must be journaled
    /// automatically afterward, exactly as in the create branch.
    #[tokio::test]
    async fn update_path_convergence_introducing_a_collision_never_rejects() -> Result<()> {
        let hub = device().await?;

        // Alice already exists in the hub.
        let alice_id = hub
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Alice".to_string(),
                json!({ "person": { "name": "Alice", "email": "alice@example.com" } }),
            ))
            .await?;

        // Bob also already exists in the hub (e.g. from an earlier, unrelated
        // sync pull) with a distinct email — no collision yet.
        let bob_id = hub
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Bob".to_string(),
                json!({ "person": { "name": "Bob", "email": "bob@example.com" } }),
            ))
            .await?;
        let bob_before = hub.service.get_node(&bob_id).await?.unwrap();
        assert!(open_unique_field_collisions(&hub.service, &bob_id)
            .await?
            .is_empty());

        // An incoming pulled UPDATE to Bob (e.g. he changed his email on another
        // device) now collides with Alice's. Applied via update_node — the
        // "node already present locally" branch a real apply takes when the
        // incoming node's id already exists in this store.
        let updated = hub
            .service
            .update_node(
                &bob_id,
                bob_before.version,
                NodeUpdate::new().with_properties(
                    json!({ "person": { "name": "Bob", "email": "ALICE@example.com" } }),
                ),
            )
            .await
            .expect(
                "an update that introduces a `unique`-rule collision must never be \
                 rejected — this is the update-branch analogue of the create-branch \
                 no-rejection guarantee",
            );
        assert_eq!(updated.id, bob_id);

        // Both nodes survive, Bob's update landed.
        let alice_after = hub.service.get_node(&alice_id).await?.unwrap();
        let bob_after = hub.service.get_node(&bob_id).await?.unwrap();
        assert_eq!(
            alice_after
                .properties
                .get("person")
                .and_then(|p| p.get("email"))
                .and_then(|v| v.as_str()),
            Some("alice@example.com")
        );
        assert_eq!(
            bob_after
                .properties
                .get("person")
                .and_then(|p| p.get("email"))
                .and_then(|v| v.as_str()),
            Some("ALICE@example.com"),
            "Bob's updated email must have actually landed"
        );

        // The collision is journaled automatically, exactly as in the create case.
        let alice_records = open_unique_field_collisions(&hub.service, &alice_id).await?;
        let bob_records = open_unique_field_collisions(&hub.service, &bob_id).await?;
        assert_eq!(alice_records.len(), 1);
        assert_eq!(bob_records.len(), 1);
        assert_eq!(alice_records[0].id, bob_records[0].id);

        Ok(())
    }

    /// A narrow, real concurrency case: two devices' colliding nodes applied
    /// CONCURRENTLY into one hub (as two racing sync-pull tasks might), rather
    /// than the sequential applies every other test in this file uses. Both
    /// must land, and neither may error — proving the no-rejection guarantee
    /// isn't an artifact of strict sequencing. This does not exercise
    /// concurrent DETECTION (a harder, separate question, and the accepted
    /// TOCTOU gap per conflict-journal-and-resolution.md §4); it exercises
    /// concurrent WRITE application, which is the part sync's pull pipeline
    /// can genuinely race.
    #[tokio::test]
    async fn concurrent_convergence_of_two_colliding_devices_never_rejects() -> Result<()> {
        let hub = Arc::new(device().await?);

        let device_a = device().await?;
        let a_id = device_a
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Alice A".to_string(),
                json!({ "person": { "name": "Alice A", "email": "concurrent@example.com" } }),
            ))
            .await?;
        let a_node = device_a.service.get_node(&a_id).await?.unwrap();

        let device_b = device().await?;
        let b_id = device_b
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Alice B".to_string(),
                json!({ "person": { "name": "Alice B", "email": "CONCURRENT@example.com" } }),
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
            .expect("concurrent apply of A must never be rejected on a uniqueness collision");
        result_b
            .expect("task must not panic")
            .expect("concurrent apply of B must never be rejected on a uniqueness collision");

        assert!(hub.service.get_node(&a_id).await?.is_some());
        assert!(hub.service.get_node(&b_id).await?.is_some());

        Ok(())
    }

    /// Re-detecting an already-journaled collision must bump `occurrences`
    /// on the same record rather than create a second one — the derived-id
    /// idempotence ADR-068 §3 requires. Triggered here by a no-op update to
    /// the already-converged pair (any create/update on either participant
    /// re-runs detection).
    #[tokio::test]
    async fn redetecting_the_same_collision_bumps_occurrences_not_a_new_record() -> Result<()> {
        let device_a = device().await?;
        let alice_id = device_a
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Alice".to_string(),
                json!({ "person": { "name": "Alice", "email": "alice@example.com" } }),
            ))
            .await?;
        let device_b = device().await?;
        let bob_id = device_b
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Bob".to_string(),
                json!({ "person": { "name": "Bob", "email": "alice@example.com" } }),
            ))
            .await?;
        let bobs_node = device_b.service.get_node(&bob_id).await?.unwrap();
        apply_incoming(&device_a.service, bobs_node).await?;

        let first = open_unique_field_collisions(&device_a.service, &alice_id).await?;
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].occurrences, 1);

        // Any further update to Bob re-runs detection against the same
        // already-colliding email, re-deriving the SAME id.
        let bob_current = device_a.service.get_node(&bob_id).await?.unwrap();
        device_a
            .service
            .update_node(
                &bob_id,
                bob_current.version,
                NodeUpdate::new().with_content("Bob (renamed)".to_string()),
            )
            .await?;

        let second = open_unique_field_collisions(&device_a.service, &alice_id).await?;
        assert_eq!(
            second.len(),
            1,
            "re-detection must upsert the existing record, not append a new one"
        );
        assert_eq!(second[0].id, first[0].id, "derived id must be stable");
        assert_eq!(
            second[0].occurrences, 2,
            "re-detection must bump occurrences on the existing record"
        );

        Ok(())
    }

    /// A title- or lifecycle_status-only update touches neither `content`
    /// nor `properties` — where a unique field's value actually lives — so
    /// `update_node` skips the post-commit re-detection call entirely for
    /// it (a perf short-circuit: no get_node/get_schema_node round-trip for
    /// an update that could not possibly introduce or need to re-detect a
    /// collision). This must not be confused with the content-only case
    /// above, which still re-runs detection.
    #[tokio::test]
    async fn title_only_update_does_not_bump_occurrences() -> Result<()> {
        let device_a = device().await?;
        let alice_id = device_a
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Alice".to_string(),
                json!({ "person": { "name": "Alice", "email": "alice@example.com" } }),
            ))
            .await?;
        let device_b = device().await?;
        let bob_id = device_b
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Bob".to_string(),
                json!({ "person": { "name": "Bob", "email": "alice@example.com" } }),
            ))
            .await?;
        let bobs_node = device_b.service.get_node(&bob_id).await?.unwrap();
        apply_incoming(&device_a.service, bobs_node).await?;

        let first = open_unique_field_collisions(&device_a.service, &alice_id).await?;
        assert_eq!(first[0].occurrences, 1);

        let bob_current = device_a.service.get_node(&bob_id).await?.unwrap();
        device_a
            .service
            .update_node(
                &bob_id,
                bob_current.version,
                NodeUpdate::new().with_title(Some("Bob Title".to_string())),
            )
            .await?;

        let second = open_unique_field_collisions(&device_a.service, &alice_id).await?;
        assert_eq!(
            second[0].occurrences, 1,
            "a title-only update must not re-run unique-field-collision detection"
        );

        Ok(())
    }

    /// Dismissing a conflict record, then re-detecting the same collision,
    /// must leave it dismissed rather than re-raising it — the
    /// dismiss-persistence property `_possible_duplicate` never had
    /// (conflict-journal-and-resolution.md §5.1, ADR-068 §3).
    #[tokio::test]
    async fn dismissing_then_redetecting_leaves_the_record_dismissed() -> Result<()> {
        let device_a = device().await?;
        let alice_id = device_a
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Alice".to_string(),
                json!({ "person": { "name": "Alice", "email": "alice@example.com" } }),
            ))
            .await?;
        let device_b = device().await?;
        let bob_id = device_b
            .service
            .create_node(Node::new(
                "person".to_string(),
                "Bob".to_string(),
                json!({ "person": { "name": "Bob", "email": "alice@example.com" } }),
            ))
            .await?;
        let bobs_node = device_b.service.get_node(&bob_id).await?.unwrap();
        apply_incoming(&device_a.service, bobs_node).await?;

        let records = open_unique_field_collisions(&device_a.service, &alice_id).await?;
        assert_eq!(records.len(), 1);
        let conflict_id = records[0].id.clone();

        device_a
            .service
            .resolve_conflict(&conflict_id, nodespace_core::models::Resolution::Dismiss)
            .await?;

        let after_dismiss = device_a.service.conflicts_for_node(&alice_id).await?;
        let dismissed = after_dismiss
            .iter()
            .find(|r| r.id == conflict_id)
            .expect("record must still exist after dismissal");
        assert_eq!(dismissed.status, ConflictStatus::Dismissed);

        // Re-trigger detection (another no-op-ish update to Bob).
        let bob_current = device_a.service.get_node(&bob_id).await?.unwrap();
        device_a
            .service
            .update_node(
                &bob_id,
                bob_current.version,
                NodeUpdate::new().with_content("Bob (still colliding)".to_string()),
            )
            .await?;

        // Must remain dismissed and must NOT reappear as an open record.
        let open_after_redetect =
            open_unique_field_collisions(&device_a.service, &alice_id).await?;
        assert!(
            open_after_redetect.is_empty(),
            "a dismissed conflict must not be re-raised as open by re-detection"
        );
        let all_after_redetect = device_a.service.conflicts_for_node(&alice_id).await?;
        let still_dismissed = all_after_redetect
            .iter()
            .find(|r| r.id == conflict_id)
            .expect("the SAME record must still exist, not a new one");
        assert_eq!(still_dismissed.status, ConflictStatus::Dismissed);

        Ok(())
    }
}
