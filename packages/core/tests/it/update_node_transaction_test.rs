//! Regression coverage for `SqliteStore::update_node`'s transactional
//! atomicity, and its version/modified_at bump guarantee regardless of which
//! of content/title/lifecycle_status actually changed.

use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, NodeUpdate};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn create_test_store() -> (Arc<SqliteStore>, TempDir, std::path::PathBuf) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let store = Arc::new(SqliteStore::new(db_path.clone()).await.unwrap());
    (store, temp_dir, db_path)
}

/// A second, independent connection to the SAME sqlite file — used only to
/// inject a schema constraint the store's own writes must trip over, so a
/// mid-update failure can be forced deterministically through the public API
/// instead of needing a fault-injection seam inside the store itself.
async fn open_raw(db_path: &std::path::Path) -> libsql::Connection {
    nodespace_core::db::ensure_sqlite_vec_registered().await;
    let database = libsql::Builder::new_local(db_path)
        .build()
        .await
        .expect("build libsql database");
    database.connect().expect("connect")
}

/// A failure partway through `update_node`'s several UPDATE statements must
/// leave NO partial state behind: content and title/version/modified_at
/// either all move together or none of them do.
///
/// Forces the failure deterministically by making the title-update statement
/// (which runs AFTER the content/properties statement) violate a UNIQUE
/// index. Before the transaction wrap, the content/properties statement would
/// already have autocommitted and durably persisted by the time the title
/// statement failed — this test would then see the new content survive
/// alongside the old title, i.e. exactly the torn state the issue describes.
#[tokio::test]
async fn update_node_failure_partway_through_leaves_no_partial_state() {
    let (store, _tmp, db_path) = create_test_store().await;

    let target = Node::new(
        "text".to_string(),
        "original content".to_string(),
        json!({}),
    );
    let target_id = target.id.clone();
    store.create_node(target, None, None).await.unwrap();

    // A different node already owns the title we're about to collide with.
    let other = Node::new("text".to_string(), "other content".to_string(), json!({}));
    let other_id = other.id.clone();
    store.create_node(other, None, None).await.unwrap();
    store
        .update_node(
            &other_id,
            NodeUpdate::new().with_title(Some("taken-title".to_string())),
            None,
        )
        .await
        .unwrap();

    let before = store.get_node(&target_id).await.unwrap().unwrap();

    let raw = open_raw(&db_path).await;
    raw.execute("CREATE UNIQUE INDEX ux_test_node_title ON node(title)", ())
        .await
        .expect("create unique index");
    drop(raw);

    let update = NodeUpdate::new()
        .with_content("new content that must NOT persist".to_string())
        .with_title(Some("taken-title".to_string()));

    let result = store.update_node(&target_id, update, None).await;
    assert!(
        result.is_err(),
        "the update must fail outright — the requested title collides with the unique index"
    );

    let after = store.get_node(&target_id).await.unwrap().unwrap();
    assert_eq!(
        after.content, before.content,
        "content must roll back — it must never persist without the title change that was \
         requested alongside it in the same logical update"
    );
    assert_eq!(after.title, before.title, "title must be unchanged");
    assert_eq!(
        after.version, before.version,
        "version must not bump on a failed update"
    );
    assert_eq!(
        after.modified_at, before.modified_at,
        "modified_at must not bump on a failed update"
    );
}

/// A title-only update (no content/node_type/properties change) must bump
/// `version` and `modified_at` exactly like a content update does — otherwise
/// optimistic-concurrency clients and modified_at-based staleness/sync checks
/// never see a title-only change land.
#[tokio::test]
async fn title_only_update_bumps_version_and_modified_at() {
    let (store, _tmp, _path) = create_test_store().await;

    let node = Node::new("text".to_string(), "content".to_string(), json!({}));
    let id = node.id.clone();
    store.create_node(node, None, None).await.unwrap();

    let before = store.get_node(&id).await.unwrap().unwrap();

    store
        .update_node(
            &id,
            NodeUpdate::new().with_title(Some("brand new title".to_string())),
            None,
        )
        .await
        .unwrap();

    let after = store.get_node(&id).await.unwrap().unwrap();
    assert_eq!(after.title, Some("brand new title".to_string()));
    assert_eq!(
        after.version,
        before.version + 1,
        "a title-only update must bump version exactly like a content update does"
    );
    assert!(
        after.modified_at > before.modified_at,
        "a title-only update must bump modified_at exactly like a content update does"
    );
}

/// Same guarantee, for a lifecycle_status-only update.
#[tokio::test]
async fn lifecycle_status_only_update_bumps_version_and_modified_at() {
    let (store, _tmp, _path) = create_test_store().await;

    let node = Node::new("text".to_string(), "content".to_string(), json!({}));
    let id = node.id.clone();
    store.create_node(node, None, None).await.unwrap();

    let before = store.get_node(&id).await.unwrap().unwrap();

    store
        .update_node(
            &id,
            NodeUpdate::new().with_lifecycle_status("archived".to_string()),
            None,
        )
        .await
        .unwrap();

    let after = store.get_node(&id).await.unwrap().unwrap();
    assert_eq!(after.lifecycle_status, "archived");
    assert_eq!(
        after.version,
        before.version + 1,
        "a lifecycle_status-only update must bump version exactly like a content update does"
    );
    assert!(
        after.modified_at > before.modified_at,
        "a lifecycle_status-only update must bump modified_at exactly like a content update does"
    );
}
