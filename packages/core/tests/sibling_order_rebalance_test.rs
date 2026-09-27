//! Repeatedly placing a node right after the same anchor sibling halves the
//! order gap between the anchor and its successor every time. Without a
//! rebalance the midpoint eventually rounds onto an existing key and two
//! `has_child` edges share one order — the sibling order becomes undefined.
//! Both write paths that insert between siblings — `create_node_with_parent`
//! and `move_node` — must re-spread the siblings before that happens.
//!
//! A re-spread rewrites every sibling's key, so both paths must also announce
//! those rewrites: a client that keeps sibling order from events alone would
//! otherwise slot the new edge's key among stale ones.

use nodespace_core::db::events::DomainEvent;
use nodespace_core::db::SqliteStore;
use nodespace_core::services::{
    CreateNodeParams, InsertPosition, InsertPositionOwned, NodeService,
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

/// Far more than the ~50 halvings it takes an f64 gap of 1.0 to collapse.
const INSERTS: usize = 60;

fn params(content: &str, parent: Option<&str>, position: InsertPositionOwned) -> CreateNodeParams {
    CreateNodeParams {
        id: None,
        node_type: "text".to_string(),
        content: content.to_string(),
        parent_id: parent.map(str::to_string),
        position,
        properties: serde_json::json!({}),
        lifecycle_status: None,
    }
}

struct Fixture {
    service: NodeService,
    conn: libsql::Connection,
    parent: String,
    anchor: String,
    _temp_dir: TempDir,
}

/// A parent with two children, `anchor` then `tail`. The anchor's edge carries
/// an extra `label` property: a re-spread rewrites only `order`, so it must
/// survive.
async fn fixture() -> Fixture {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path.clone()).await.unwrap());
    let service = NodeService::new(&mut store).await.unwrap();

    let parent = service
        .create_node_with_parent(params("parent", None, InsertPositionOwned::End))
        .await
        .unwrap();
    let anchor = service
        .create_node_with_parent(params("anchor", Some(&parent), InsertPositionOwned::End))
        .await
        .unwrap();
    service
        .create_node_with_parent(params("tail", Some(&parent), InsertPositionOwned::End))
        .await
        .unwrap();

    nodespace_core::db::ensure_sqlite_vec_registered().await;
    let conn = libsql::Builder::new_local(&db_path)
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    conn.execute(
        "UPDATE relationship SET properties = json_set(properties, '$.label', 'keep') WHERE out_node = ?1",
        libsql::params![anchor.clone()],
    )
    .await
    .unwrap();

    Fixture {
        service,
        conn,
        parent,
        anchor,
        _temp_dir: temp_dir,
    }
}

/// Keys are distinct, the anchor's extra edge property survived, and the
/// children read back as anchor, newest placement … oldest placement, tail.
async fn assert_order_intact(f: &Fixture) {
    let mut rows = f
        .conn
        .query(
            "SELECT json_extract(properties, '$.order') FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child'",
            libsql::params![f.parent.clone()],
        )
        .await
        .unwrap();
    let mut orders: Vec<f64> = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        orders.push(row.get(0).unwrap());
    }
    assert_eq!(orders.len(), INSERTS + 2);
    orders.sort_by(f64::total_cmp);
    orders.dedup();
    assert_eq!(orders.len(), INSERTS + 2, "duplicate sibling order keys");

    let mut rows = f
        .conn
        .query(
            "SELECT json_extract(properties, '$.label') FROM relationship WHERE out_node = ?1",
            libsql::params![f.anchor.clone()],
        )
        .await
        .unwrap();
    let label: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(label, "keep");

    let contents: Vec<String> = f
        .service
        .get_children(&f.parent)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.content)
        .collect();
    let mut expected = vec!["anchor".to_string()];
    expected.extend((0..INSERTS).rev().map(|i| format!("item-{i}")));
    expected.push("tail".to_string());
    assert_eq!(contents, expected);
}

/// A client that learns the parent's child order from events alone, the way the
/// frontend's `hierarchy-sync` does: seeded from the store, then updated from
/// each `has_child` event's `order`.
struct EventMirror {
    rx: broadcast::Receiver<nodespace_core::db::events::EventEnvelope>,
    orders: HashMap<String, f64>,
}

impl EventMirror {
    async fn new(f: &Fixture) -> Self {
        let rx = f.service.subscribe_to_events();
        let mut rows = f
            .conn
            .query(
                "SELECT out_node, json_extract(properties, '$.order') FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child'",
                libsql::params![f.parent.clone()],
            )
            .await
            .unwrap();
        let mut orders = HashMap::new();
        while let Some(row) = rows.next().await.unwrap() {
            orders.insert(row.get::<String>(0).unwrap(), row.get::<f64>(1).unwrap());
        }
        Self { rx, orders }
    }

    /// Apply every event emitted so far, then assert the mirrored order
    /// matches the store's.
    async fn assert_matches_store(&mut self, f: &Fixture) {
        let parent = format!("node:{}", f.parent);
        loop {
            let envelope = match self.rx.try_recv() {
                Ok(envelope) => envelope,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(e) => panic!("event stream broke: {e}"),
            };
            let rel = match envelope.event {
                DomainEvent::RelationshipCreated { relationship }
                | DomainEvent::RelationshipUpdated { relationship } => relationship,
                _ => continue,
            };
            if rel.relationship_type != "has_child" || rel.from_id != parent {
                continue;
            }
            let child = rel.to_id.strip_prefix("node:").unwrap().to_string();
            self.orders
                .insert(child, rel.properties["order"].as_f64().unwrap());
        }

        let mut mirrored: Vec<(&String, &f64)> = self.orders.iter().collect();
        mirrored.sort_by(|a, b| a.1.total_cmp(b.1));
        let mirrored: Vec<&String> = mirrored.into_iter().map(|(id, _)| id).collect();
        let stored: Vec<String> = f
            .service
            .get_children(&f.parent)
            .await
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(
            mirrored,
            stored.iter().collect::<Vec<_>>(),
            "event-only client order diverged from the store"
        );
    }
}

#[tokio::test]
async fn repeated_creates_after_one_anchor_keep_distinct_ordered_keys() {
    let f = fixture().await;
    for i in 0..INSERTS {
        f.service
            .create_node_with_parent(params(
                &format!("item-{i}"),
                Some(&f.parent),
                InsertPositionOwned::After(f.anchor.clone()),
            ))
            .await
            .unwrap();
    }
    assert_order_intact(&f).await;
}

#[tokio::test]
async fn repeated_moves_after_one_anchor_keep_distinct_ordered_keys() {
    let f = fixture().await;
    let mut items = Vec::new();
    for i in 0..INSERTS {
        let id = f
            .service
            .create_node_with_parent(params(
                &format!("item-{i}"),
                Some(&f.parent),
                InsertPositionOwned::End,
            ))
            .await
            .unwrap();
        items.push(id);
    }
    // A same-parent reorder rewrites only the moved edge's `order`, too.
    f.conn
        .execute(
            "UPDATE relationship SET properties = json_set(properties, '$.label', 'moved') WHERE out_node = ?1",
            libsql::params![items[0].clone()],
        )
        .await
        .unwrap();
    for id in &items {
        let version = f.service.get_node(id).await.unwrap().unwrap().version;
        f.service
            .move_node(
                id,
                version,
                Some(&f.parent),
                InsertPosition::After(&f.anchor),
            )
            .await
            .unwrap();
    }
    assert_order_intact(&f).await;

    let mut rows = f
        .conn
        .query(
            "SELECT json_extract(properties, '$.label') FROM relationship WHERE out_node = ?1",
            libsql::params![items[0].clone()],
        )
        .await
        .unwrap();
    let label: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(label, "moved");
}

#[tokio::test]
async fn creates_that_respread_leave_an_event_only_client_in_store_order() {
    let f = fixture().await;
    let mut mirror = EventMirror::new(&f).await;
    for i in 0..INSERTS {
        f.service
            .create_node_with_parent(params(
                &format!("item-{i}"),
                Some(&f.parent),
                InsertPositionOwned::After(f.anchor.clone()),
            ))
            .await
            .unwrap();
        mirror.assert_matches_store(&f).await;
    }
}

#[tokio::test]
async fn moves_that_respread_leave_an_event_only_client_in_store_order() {
    let f = fixture().await;
    let mut items = Vec::new();
    for i in 0..INSERTS {
        let id = f
            .service
            .create_node_with_parent(params(
                &format!("item-{i}"),
                Some(&f.parent),
                InsertPositionOwned::End,
            ))
            .await
            .unwrap();
        items.push(id);
    }
    let mut mirror = EventMirror::new(&f).await;
    for id in &items {
        let version = f.service.get_node(id).await.unwrap().unwrap().version;
        f.service
            .move_node(
                id,
                version,
                Some(&f.parent),
                InsertPosition::After(&f.anchor),
            )
            .await
            .unwrap();
        mirror.assert_matches_store(&f).await;
    }
}

/// Every `has_child` key under the fixture's parent, by child id.
async fn sibling_keys(f: &Fixture) -> HashMap<String, f64> {
    let mut rows = f
        .conn
        .query(
            "SELECT out_node, json_extract(properties, '$.order') FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child'",
            libsql::params![f.parent.clone()],
        )
        .await
        .unwrap();
    let mut keys = HashMap::new();
    while let Some(row) = rows.next().await.unwrap() {
        keys.insert(row.get::<String>(0).unwrap(), row.get::<f64>(1).unwrap());
    }
    keys
}

/// Move `mover` after the anchor with the anchor → next-sibling gap closed
/// below `MIN_GAP`, so the move re-spreads the parent's children before its
/// edge write — which the caller's `fail_move_edge` trigger makes fail. The
/// re-spread must roll back with it: every sibling keeps its key, and an
/// event-only client still matches the store.
async fn assert_failed_move_leaves_siblings_unchanged(f: &Fixture, mover: &str) {
    let next = f.service.get_children(&f.parent).await.unwrap()[1]
        .id
        .clone();
    f.conn
        .execute(
            "UPDATE relationship SET properties = json_set(properties, '$.order', \
               (SELECT json_extract(properties, '$.order') + 0.00001 FROM relationship \
                WHERE out_node = ?1 AND relationship_type = 'has_child')) \
             WHERE out_node = ?2 AND relationship_type = 'has_child'",
            libsql::params![f.anchor.clone(), next],
        )
        .await
        .unwrap();

    let before = sibling_keys(f).await;
    let mut mirror = EventMirror::new(f).await;
    let version = f.service.get_node(mover).await.unwrap().unwrap().version;
    let result = f
        .service
        .move_node(
            mover,
            version,
            Some(&f.parent),
            InsertPosition::After(&f.anchor),
        )
        .await;
    assert!(
        result.is_err(),
        "the injected edge-write failure must fail the move"
    );
    assert_eq!(
        sibling_keys(f).await,
        before,
        "a failed move left rewritten sibling keys"
    );
    mirror.assert_matches_store(f).await;
}

#[tokio::test]
async fn failed_same_parent_move_rolls_back_its_respread() {
    let f = fixture().await;
    let mover = f
        .service
        .create_node_with_parent(params("mover", Some(&f.parent), InsertPositionOwned::End))
        .await
        .unwrap();
    // The re-spread rewrites `order` only; the reorder's own UPDATE also bumps
    // the edge's version, so this fails that UPDATE and nothing before it.
    f.conn
        .execute(
            &format!(
                "CREATE TRIGGER fail_move_edge BEFORE UPDATE ON relationship \
                 WHEN NEW.out_node = '{mover}' AND NEW.version <> OLD.version \
                 BEGIN SELECT RAISE(ABORT, 'injected move failure'); END"
            ),
            (),
        )
        .await
        .unwrap();
    assert_failed_move_leaves_siblings_unchanged(&f, &mover).await;
}

#[tokio::test]
async fn failed_cross_parent_move_rolls_back_its_respread() {
    let f = fixture().await;
    let other = f
        .service
        .create_node_with_parent(params("other", None, InsertPositionOwned::End))
        .await
        .unwrap();
    let mover = f
        .service
        .create_node_with_parent(params("mover", Some(&other), InsertPositionOwned::End))
        .await
        .unwrap();
    f.conn
        .execute(
            &format!(
                "CREATE TRIGGER fail_move_edge BEFORE INSERT ON relationship \
                 WHEN NEW.out_node = '{mover}' \
                 BEGIN SELECT RAISE(ABORT, 'injected move failure'); END"
            ),
            (),
        )
        .await
        .unwrap();
    assert_failed_move_leaves_siblings_unchanged(&f, &mover).await;

    // The old edge's DELETE rolled back with the rest: still under `other`.
    let children: Vec<String> = f
        .service
        .get_children(&other)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(children, vec![mover]);
}
