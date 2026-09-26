//! Repeatedly inserting a new child right after the same anchor sibling halves
//! the order gap between the anchor and its successor on every insert. Without
//! a rebalance the midpoint eventually rounds onto an existing key and two
//! `has_child` edges share one order — the sibling order becomes undefined.
//! `create_node_with_parent` must re-spread the siblings before that happens,
//! the same safeguard `move_node` applies.

use nodespace_core::db::SqliteStore;
use nodespace_core::services::{CreateNodeParams, InsertPositionOwned, NodeService};
use std::sync::Arc;
use tempfile::TempDir;

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

#[tokio::test]
async fn repeated_inserts_after_one_anchor_keep_distinct_ordered_keys() {
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

    // Far more than the ~50 halvings it takes an f64 gap of 1.0 to collapse.
    const INSERTS: usize = 60;
    for i in 0..INSERTS {
        service
            .create_node_with_parent(params(
                &format!("item-{i}"),
                Some(&parent),
                InsertPositionOwned::After(anchor.clone()),
            ))
            .await
            .unwrap();
    }

    nodespace_core::db::ensure_sqlite_vec_registered().await;
    let conn = libsql::Builder::new_local(&db_path)
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap();
    let mut rows = conn
        .query(
            "SELECT json_extract(properties, '$.order') FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child'",
            libsql::params![parent.clone()],
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

    // Each insert lands immediately after the anchor, so the newest is first.
    let contents: Vec<String> = service
        .get_children(&parent)
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
