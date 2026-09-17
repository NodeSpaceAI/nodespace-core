//! Coverage for `relationship.reverse_relationship_type`: the name an edge
//! reads by from the target's end, stored as a queryable column rather than
//! buried in the row's `properties` JSON (or, for the built-in structural
//! relationships, not stored at all).
//!
//! The load-bearing assertion here is the last one: **no edge may reach the
//! table without its reverse name**. The column is only useful if it is
//! populated by every write path, and there are many of them spread across
//! three modules — a reverse-traversal query that silently misses rows written
//! by one forgotten path is worse than no column at all.

use nodespace_core::db::SqliteStore;
use nodespace_core::models::schema::{builtin_reverse_name, BUILTIN_RELATIONSHIPS};
use nodespace_core::services::{CreateNodeParams, InsertPositionOwned, NodeService};
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

/// A service plus a raw connection to the same file, so assertions can read the
/// column directly — the store's own `read()` is crate-private.
async fn test_service() -> (NodeService, TempDir, libsql::Connection) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path.clone()).await.unwrap());
    let service = NodeService::new(&mut store).await.unwrap();
    let conn = raw_conn(&db_path).await;
    (service, temp_dir, conn)
}

async fn raw_conn(db_path: &Path) -> libsql::Connection {
    nodespace_core::db::ensure_sqlite_vec_registered().await;
    libsql::Builder::new_local(db_path)
        .build()
        .await
        .unwrap()
        .connect()
        .unwrap()
}

fn params(content: &str, parent: Option<&str>) -> CreateNodeParams {
    CreateNodeParams {
        id: None,
        node_type: "text".to_string(),
        content: content.to_string(),
        parent_id: parent.map(str::to_string),
        position: InsertPositionOwned::End,
        properties: serde_json::json!({}),
        lifecycle_status: None,
    }
}

/// Every built-in structural relationship has exactly one reverse name, and
/// `builtin_reverse_name` is the only place it lives.
#[test]
fn builtin_reverse_names_are_defined_and_unambiguous() {
    assert_eq!(builtin_reverse_name("has_child"), Some("child_of"));
    assert_eq!(builtin_reverse_name("member_of"), Some("has_member"));
    assert_eq!(builtin_reverse_name("mentions"), Some("mentioned_by"));
    assert_eq!(builtin_reverse_name("has_role"), Some("role_of"));

    // A schema-declared relationship's reverse name comes from its own
    // declaration, never from the built-in table.
    assert_eq!(builtin_reverse_name("tasks"), None);
    assert_eq!(builtin_reverse_name("extends"), None);

    let forwards: Vec<&str> = BUILTIN_RELATIONSHIPS.iter().map(|(f, _)| *f).collect();
    let reverses: Vec<&str> = BUILTIN_RELATIONSHIPS.iter().map(|(_, r)| *r).collect();

    // No forward may map to two reverses and no reverse may be claimed by two
    // forwards — either would make reverse traversal ambiguous.
    for names in [&forwards, &reverses] {
        let mut sorted = (*names).clone();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), before, "duplicate name in {names:?}");
    }

    // A reverse name must not collide with any forward name, or filtering by
    // reverse name would also match real forward edges.
    for reverse in &reverses {
        assert!(
            !forwards.contains(reverse),
            "reverse name {reverse} collides with a forward relationship name"
        );
    }
}

/// A `has_child` edge written through the ordinary node-creation path carries
/// `child_of`, readable as a column without decoding any JSON.
#[tokio::test]
async fn has_child_edges_store_child_of_as_a_queryable_column() {
    let (service, _tmp, conn) = test_service().await;

    let parent = service
        .create_node_with_parent(params("parent", None))
        .await
        .unwrap();
    let child = service
        .create_node_with_parent(params("child", Some(&parent)))
        .await
        .unwrap();

    let mut rows = conn
        .query(
            "SELECT reverse_relationship_type FROM relationship \
             WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = 'has_child'",
            libsql::params![parent.clone(), child.clone()],
        )
        .await
        .unwrap();

    let reverse: String = rows
        .next()
        .await
        .unwrap()
        .expect("the has_child edge must exist")
        .get(0)
        .unwrap();
    assert_eq!(reverse, "child_of");
}

/// Reverse traversal by name: "which node is this one a `child_of`" becomes a
/// column lookup, which is the capability this column exists to enable.
#[tokio::test]
async fn an_edge_is_findable_from_the_target_end_by_its_reverse_name() {
    let (service, _tmp, conn) = test_service().await;

    let parent = service
        .create_node_with_parent(params("parent", None))
        .await
        .unwrap();
    let child = service
        .create_node_with_parent(params("child", Some(&parent)))
        .await
        .unwrap();

    let mut rows = conn
        .query(
            "SELECT in_node FROM relationship \
             WHERE out_node = ?1 AND reverse_relationship_type = 'child_of'",
            libsql::params![child.clone()],
        )
        .await
        .unwrap();

    let found: String = rows
        .next()
        .await
        .unwrap()
        .expect("child_of must resolve to the parent from the child's end")
        .get(0)
        .unwrap();
    assert_eq!(found, parent);
}

/// A schema-declared relationship stores the author's own `reverse_name` — not
/// anything derived — so the column reads identically for declared and
/// built-in edges.
#[tokio::test]
async fn schema_declared_edges_store_the_authors_reverse_name() {
    let (_service, _tmp, conn) = test_service().await;

    let mut rows = conn
        .query(
            "SELECT relationship_type, reverse_relationship_type FROM relationship \
             WHERE reverse_relationship_type IS NOT NULL \
               AND relationship_type NOT IN ('member_of', 'has_child', 'mentions', 'has_role') \
             LIMIT 5",
            (),
        )
        .await
        .unwrap();

    let mut seen = 0;
    while let Some(row) = rows.next().await.unwrap() {
        let forward: String = row.get(0).unwrap();
        let reverse: String = row.get(1).unwrap();
        assert!(
            !reverse.trim().is_empty(),
            "declared relationship {forward} stored an empty reverse name"
        );
        assert_eq!(
            builtin_reverse_name(&forward),
            None,
            "{forward} is not a built-in, so its reverse must come from its declaration"
        );
        seen += 1;
    }
    assert!(
        seen > 0,
        "core schemas should declare at least one relationship with a reverse name"
    );
}

/// **The completeness guard.** Every edge in the table must carry a reverse
/// name, whichever of the many write paths created it. A single unpopulated row
/// means some path was missed, and reverse traversal would silently skip
/// exactly the edges that path produces.
#[tokio::test]
async fn no_write_path_leaves_the_reverse_name_unpopulated() {
    let (service, _tmp, conn) = test_service().await;

    // Exercise a spread of distinct write paths, not just one. Core schema
    // seeding has already run by this point, so declared edges are covered too.
    let parent = service
        .create_node_with_parent(params("parent", None))
        .await
        .unwrap();
    let child = service
        .create_node_with_parent(params("child", Some(&parent)))
        .await
        .unwrap();
    // Nested, so the subtree/sibling paths are covered as well.
    service
        .create_node_with_parent(params("grandchild", Some(&child)))
        .await
        .unwrap();
    // A generic edge through the dynamic-type path.
    let other = service
        .create_node_with_parent(params("other", None))
        .await
        .unwrap();
    service
        .create_relationship(&parent, "mentions", &other, serde_json::json!({}))
        .await
        .ok();

    let mut rows = conn
        .query(
            "SELECT relationship_type, count(*) FROM relationship \
             WHERE reverse_relationship_type IS NULL OR trim(reverse_relationship_type) = '' \
             GROUP BY relationship_type",
            (),
        )
        .await
        .unwrap();

    let mut offenders = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        let rel_type: String = row.get(0).unwrap();
        let count: i64 = row.get(1).unwrap();
        offenders.push(format!("{rel_type} ({count} rows)"));
    }

    assert!(
        offenders.is_empty(),
        "every relationship row must carry a reverse name; these write paths left it unset: {}",
        offenders.join(", ")
    );
}
