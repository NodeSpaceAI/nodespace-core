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

    // Deliberately NOT filtered on `IS NOT NULL`: a declared row that regressed
    // to NULL is precisely the failure this test is named for, and filtering it
    // out would hide it.
    let mut rows = conn
        .query(
            "SELECT relationship_type, reverse_relationship_type FROM relationship \
             WHERE relationship_type NOT IN ('member_of', 'has_child', 'mentions', 'has_role')",
            (),
        )
        .await
        .unwrap();

    // The author's declared names, as the core schemas spell them.
    //
    // Keyed by forward name, so only relationships whose forward name is
    // declared ONCE across all core schemas belong here. `tasks` is excluded
    // deliberately: it is declared on both `project` (reverse `project`) and
    // `person` (reverse `assignee`), so the forward name alone does not
    // identify which declaration a row came from — the reverse name is a
    // property of the declaration, not of the name.
    let expected: std::collections::HashMap<&str, &str> = [
        ("blocks", "blocked_by"),
        ("relates_to", "related_from"),
        ("duplicates", "duplicated_by"),
    ]
    .into_iter()
    .collect();

    let mut seen = 0;
    let mut checked_against_declaration = 0;
    while let Some(row) = rows.next().await.unwrap() {
        let forward: String = row.get(0).unwrap();
        let reverse: Option<String> = row.get(1).unwrap();
        let reverse = reverse.unwrap_or_else(|| {
            panic!("declared relationship {forward} stored no reverse name at all")
        });
        assert!(
            !reverse.trim().is_empty(),
            "declared relationship {forward} stored an empty reverse name"
        );
        assert_eq!(
            builtin_reverse_name(&forward),
            None,
            "{forward} is not a built-in, so its reverse must come from its declaration"
        );
        // Where the declaration's name is known, assert the stored value IS it —
        // not merely that something non-empty was stored.
        if let Some(declared) = expected.get(forward.as_str()) {
            assert_eq!(
                &reverse, declared,
                "{forward} must store its declaration's reverse name"
            );
            checked_against_declaration += 1;
        }
        seen += 1;
    }
    assert!(
        seen > 0,
        "core schemas should declare at least one relationship with a reverse name"
    );
    assert!(
        checked_against_declaration > 0,
        "at least one row must be checked against its declaration's actual name, \
         or this test only proves something non-empty was stored"
    );
}

/// **The completeness guard.** Every edge in the table must carry a reverse
/// name, whichever of the many write paths created it. A single unpopulated row
/// means some path was missed, and reverse traversal would silently skip
/// exactly the edges that path produces.
#[tokio::test]
async fn no_write_path_leaves_the_reverse_name_unpopulated() {
    let (service, _tmp, conn) = test_service().await;

    // Drive a spread of genuinely distinct write paths. Each line below reaches
    // a different INSERT site — asserting over the whole table is only as good
    // as the paths actually exercised, so a path missing here is a path whose
    // regression would ship green.
    let parent = service
        .create_node_with_parent(params("parent", None))
        .await
        .unwrap();
    let child = service
        .create_node_with_parent(params("child", Some(&parent)))
        .await
        .unwrap();
    // Nested, so the subtree/sibling paths are covered as well.
    let grandchild = service
        .create_node_with_parent(params("grandchild", Some(&child)))
        .await
        .unwrap();

    // Re-parenting: a distinct `has_child` write from the creation path.
    let second_parent = service
        .create_node_with_parent(params("second parent", None))
        .await
        .unwrap();
    service
        .move_node_unchecked(
            &grandchild,
            Some(&second_parent),
            nodespace_core::services::InsertPosition::End,
        )
        .await
        .expect("re-parenting must succeed");

    // A built-in edge through the dynamic-type path (the `CASE`-hit branch).
    let other = service
        .create_node_with_parent(params("other", None))
        .await
        .unwrap();
    service
        .create_relationship(&parent, "mentions", &other, serde_json::json!({}))
        .await
        .ok();

    // `has_child` via create_relationship with no explicit order, which routes
    // to the auto-ordering append path rather than the generic one.
    let appended = service
        .create_node_with_parent(params("appended", None))
        .await
        .unwrap();
    service
        .create_relationship(&other, "has_child", &appended, serde_json::json!({}))
        .await
        .expect("appending a child must succeed");

    // The bulk attach path used by the sync cold sweep.
    let bulk_parent = service
        .create_node_with_parent(params("bulk parent", None))
        .await
        .unwrap();
    let bulk_child = service
        .create_node_with_parent(params("bulk child", None))
        .await
        .unwrap();
    service
        .bulk_create_has_child_edges(&[(bulk_parent, bulk_child, 1.0)])
        .await
        .expect("bulk attach must succeed");

    // Collection membership — a `member_of` write, which no other line reaches.
    let collection = service
        .create_node_with_parent(CreateNodeParams {
            node_type: "collection".to_string(),
            content: "collection".to_string(),
            ..params("collection", None)
        })
        .await
        .unwrap();
    let member = service
        .create_node_with_parent(params("member", None))
        .await
        .unwrap();
    service
        .create_relationship(&member, "member_of", &collection, serde_json::json!({}))
        .await
        .ok();

    // A DECLARED relationship's INSTANCE edge — the `CASE`-miss branch, where
    // the built-in table has nothing to offer and the reverse name has to come
    // from the declaration itself. `set_schema_declarations` writes the
    // schema→schema declaration row, NOT this task→task edge, so this is a
    // separate write path and the one most likely to be left unpopulated.
    let blocker = service
        .create_node_with_parent(CreateNodeParams {
            node_type: "task".to_string(),
            content: "blocker".to_string(),
            ..params("blocker", None)
        })
        .await
        .unwrap();
    let blocked = service
        .create_node_with_parent(CreateNodeParams {
            node_type: "task".to_string(),
            content: "blocked".to_string(),
            ..params("blocked", None)
        })
        .await
        .unwrap();
    service
        .create_relationship(&blocker, "blocks", &blocked, serde_json::json!({}))
        .await
        .expect("a declared relationship's instance edge must be creatable");

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

    // Unset is one failure mode; WRONG is the other. A built-in edge's reverse
    // must be the one its forward name maps to — a path writing some other
    // string satisfies the NULL check while still breaking reverse traversal.
    let mut wrong = conn
        .query(
            "SELECT DISTINCT relationship_type, reverse_relationship_type FROM relationship \
             WHERE relationship_type IN ('member_of', 'has_child', 'mentions', 'has_role')",
            (),
        )
        .await
        .unwrap();
    while let Some(row) = wrong.next().await.unwrap() {
        let forward: String = row.get(0).unwrap();
        let reverse: Option<String> = row.get(1).unwrap();
        let expected = builtin_reverse_name(&forward).unwrap();
        if reverse.as_deref() != Some(expected) {
            offenders.push(format!(
                "{forward} stored {:?}, expected {expected:?}",
                reverse.as_deref()
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "every relationship row must carry its correct reverse name; these write paths did not: {}",
        offenders.join(", ")
    );
}
