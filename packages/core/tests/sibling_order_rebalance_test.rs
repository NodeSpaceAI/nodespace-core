//! Repeatedly placing a node right after the same anchor sibling halves the
//! order gap between the anchor and its successor every time. Without a
//! rebalance the midpoint eventually rounds onto an existing key and two
//! `has_child` edges share one order — the sibling order becomes undefined.
//! Both write paths that insert between siblings — `create_node_with_parent`
//! and `move_node` — must re-spread the siblings before that happens.

use nodespace_core::db::SqliteStore;
use nodespace_core::services::{
    CreateNodeParams, InsertPosition, InsertPositionOwned, NodeService,
};
use std::sync::Arc;
use tempfile::TempDir;

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
