//! Store primitives that back idempotent markdown re-import.
//!
//! - `get_nodes_by_ids` must chunk under SQLite's bound-parameter ceiling so a
//!   large directory import (thousands of files) can check which roots already
//!   exist without the `IN (...)` query blowing the ~999-parameter limit.
//! - `delete_nodes_by_ids_unchecked` must delete exactly the ids it is given
//!   (and cascade their relationships), leaving every other node untouched —
//!   this is what lets a `--replace` prune the OLD subtree only after the fresh
//!   one is inserted.

use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use serde_json::json;
use tempfile::TempDir;

async fn create_test_db() -> (SqliteStore, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let store = SqliteStore::new(temp_dir.path().join("test.db"))
        .await
        .unwrap();
    (store, temp_dir)
}

fn make_node(id: &str, node_type: &str, content: &str) -> Node {
    Node::new_with_id(
        id.to_string(),
        node_type.to_string(),
        content.to_string(),
        json!({}),
    )
}

/// Querying more ids than SQLite's bound-parameter ceiling must succeed and
/// return every matching node — the query is chunked, not truncated or errored.
#[tokio::test]
async fn get_nodes_by_ids_chunks_past_the_sqlite_parameter_ceiling() {
    let (store, _tmp) = create_test_db().await;

    // Well over the ~999 bound-parameter limit / 900-row chunk size.
    let ids: Vec<String> = (0..1500).map(|i| format!("node-{i:04}")).collect();
    for id in &ids {
        store
            .create_node(make_node(id, "text", "x"), None, None)
            .await
            .unwrap();
    }

    let found = store.get_nodes_by_ids(&ids).await.unwrap();
    assert_eq!(found.len(), ids.len(), "every id must be returned");
    assert!(ids.iter().all(|id| found.contains_key(id)));
}

/// `delete_nodes_by_ids_unchecked` removes exactly the listed nodes and nothing
/// else — the property that lets a `--replace` prune an old subtree by id while
/// leaving the kept root and every unrelated document intact.
#[tokio::test]
async fn delete_nodes_by_ids_unchecked_removes_only_the_listed_ids() {
    let (store, _tmp) = create_test_db().await;

    for id in ["root", "old-0", "old-1", "old-2", "keep"] {
        store
            .create_node(make_node(id, "text", id), None, None)
            .await
            .unwrap();
    }

    // Prune two of the three "old" nodes.
    store
        .delete_nodes_by_ids_unchecked(&["old-0".to_string(), "old-1".to_string()])
        .await
        .unwrap();

    assert!(store.get_node("old-0").await.unwrap().is_none());
    assert!(store.get_node("old-1").await.unwrap().is_none());
    assert!(
        store.get_node("old-2").await.unwrap().is_some(),
        "unlisted node kept"
    );
    assert!(store.get_node("root").await.unwrap().is_some(), "root kept");
    assert!(
        store.get_node("keep").await.unwrap().is_some(),
        "unrelated node kept"
    );

    // An empty id list is a no-op, not an error.
    store.delete_nodes_by_ids_unchecked(&[]).await.unwrap();
    assert!(store.get_node("old-2").await.unwrap().is_some());
}
