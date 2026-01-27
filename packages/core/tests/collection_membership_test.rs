//! Collection Membership Tests (Issue #756)
//!
//! Integration tests for the collection system's membership operations.
//!
//! ## Collection System Overview
//!
//! Collections provide flexible, hierarchical organization for nodes:
//! - Many-to-many membership (nodes can belong to multiple collections)
//! - DAG structure (directed acyclic graph, not strictly a tree)
//! - Path-based navigation (e.g., "hr:policy:vacation")
//!
//! ## Key Concepts
//!
//! ### member_of Edge Direction
//! - Edge direction: member node → collection node
//! - Query "what collections does X belong to": SELECT ->member_of->node FROM node:X
//! - Query "what nodes are in collection Y": SELECT <-member_of<-node FROM node:Y
//!
//! ### Path Resolution
//! Collections are organized using colon-separated paths:
//! - "hr" → Top-level HR collection
//! - "hr:policy" → "policy" collection under "hr"
//! - "hr:policy:vacation" → "vacation" under "hr:policy"
//!
//! ## Test Coverage
//! - member_of edge creation and querying
//! - Path parsing and validation
//! - Adding/removing collection memberships
//! - Querying nodes by collection
//! - Collection hierarchy traversal

#[cfg(test)]
mod collection_membership_tests {
    use anyhow::Result;
    use nodespace_core::db::SurrealStore;
    use serde::Deserialize;
    use surrealdb::sql::Thing;
    use tempfile::TempDir;

    /// Test helper struct for edge results
    #[derive(Debug, Deserialize)]
    struct MemberOfEdge {
        #[serde(rename = "in")]
        in_node: Thing,
        #[serde(rename = "out")]
        out_node: Thing,
    }

    /// Helper to create test database
    async fn create_test_db() -> Result<(SurrealStore, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let store = SurrealStore::new(db_path).await?;
        Ok((store, temp_dir))
    }

    #[tokio::test]
    async fn test_member_of_edge_creation() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create a collection node
        store
            .db()
            .query(
                r#"
                CREATE node:collection1 CONTENT {
                    node_type: 'collection',
                    content: 'HR Collection',
                    data: NONE,
                    version: 1,
                    properties: { description: 'Human resources documents' },
                    created_at: time::now(),
                    modified_at: time::now()
                };
                "#,
            )
            .await?
            .check()?;

        // Create a text node
        store
            .db()
            .query(
                r#"
                CREATE node:doc1 CONTENT {
                    node_type: 'text',
                    content: 'Employee Handbook',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };
                "#,
            )
            .await?
            .check()?;

        // Create member_of edge (direction: member → collection)
        store
            .db()
            .query(
                r#"
                RELATE node:doc1->member_of->node:collection1 CONTENT {
                    created_at: time::now()
                };
                "#,
            )
            .await?
            .check()?;

        // Verify edge was created by querying outward direction
        let result = store
            .db()
            .query("SELECT * FROM member_of WHERE in = node:doc1")
            .await?;

        let mut result = result.check()?;
        let edges: Vec<MemberOfEdge> = result.take(0)?;
        assert_eq!(edges.len(), 1, "Should find one member_of edge");
        assert_eq!(edges[0].in_node.id.to_string(), "doc1");
        assert_eq!(edges[0].out_node.id.to_string(), "collection1");

        Ok(())
    }

    #[tokio::test]
    async fn test_query_node_collections() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create multiple collections
        store
            .db()
            .query(
                r#"
                CREATE node:coll_hr CONTENT {
                    node_type: 'collection',
                    content: 'HR',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };

                CREATE node:coll_policy CONTENT {
                    node_type: 'collection',
                    content: 'Policy',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };

                CREATE node:doc1 CONTENT {
                    node_type: 'text',
                    content: 'Vacation Policy',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };
                "#,
            )
            .await?
            .check()?;

        // Add doc1 to both collections
        store
            .db()
            .query(
                r#"
                RELATE node:doc1->member_of->node:coll_hr CONTENT {
                    created_at: time::now()
                };
                RELATE node:doc1->member_of->node:coll_policy CONTENT {
                    created_at: time::now()
                };
                "#,
            )
            .await?
            .check()?;

        // Query what collections doc1 belongs to (count edges instead of selecting Thing)
        let result = store
            .db()
            .query("SELECT count() FROM member_of WHERE in = node:doc1 GROUP ALL")
            .await?;

        let mut result = result.check()?;
        let count_result: Option<serde_json::Value> = result.take(0)?;
        let count = count_result.and_then(|v| v["count"].as_u64()).unwrap_or(0);
        assert_eq!(count, 2, "Document should belong to 2 collections");

        Ok(())
    }

    #[tokio::test]
    async fn test_query_collection_members() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create a collection and multiple documents
        store
            .db()
            .query(
                r#"
                CREATE node:coll_team CONTENT {
                    node_type: 'collection',
                    content: 'Team Docs',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };

                CREATE node:doc_a CONTENT {
                    node_type: 'text',
                    content: 'Document A',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };

                CREATE node:doc_b CONTENT {
                    node_type: 'text',
                    content: 'Document B',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };

                CREATE node:doc_c CONTENT {
                    node_type: 'text',
                    content: 'Document C (not in collection)',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };
                "#,
            )
            .await?
            .check()?;

        // Add doc_a and doc_b to collection (but not doc_c)
        store
            .db()
            .query(
                r#"
                RELATE node:doc_a->member_of->node:coll_team CONTENT {
                    created_at: time::now()
                };
                RELATE node:doc_b->member_of->node:coll_team CONTENT {
                    created_at: time::now()
                };
                "#,
            )
            .await?
            .check()?;

        // Query members of collection (count edges instead of selecting Thing)
        let result = store
            .db()
            .query("SELECT count() FROM member_of WHERE out = node:coll_team GROUP ALL")
            .await?;

        let mut result = result.check()?;
        let count_result: Option<serde_json::Value> = result.take(0)?;
        let count = count_result.and_then(|v| v["count"].as_u64()).unwrap_or(0);
        assert_eq!(count, 2, "Collection should have 2 members");

        Ok(())
    }

    #[tokio::test]
    async fn test_remove_membership() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create collection and document
        store
            .db()
            .query(
                r#"
                CREATE node:coll_remove CONTENT {
                    node_type: 'collection',
                    content: 'Collection for removal test',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };

                CREATE node:doc_remove CONTENT {
                    node_type: 'text',
                    content: 'Document to remove',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };
                "#,
            )
            .await?
            .check()?;

        // Add membership
        store
            .db()
            .query(
                r#"
                RELATE node:doc_remove->member_of->node:coll_remove CONTENT {
                    created_at: time::now()
                };
                "#,
            )
            .await?
            .check()?;

        // Verify membership exists (count edges)
        let result = store
            .db()
            .query("SELECT count() FROM member_of WHERE in = node:doc_remove GROUP ALL")
            .await?;

        let mut result = result.check()?;
        let count_result: Option<serde_json::Value> = result.take(0)?;
        let count = count_result.and_then(|v| v["count"].as_u64()).unwrap_or(0);
        assert_eq!(count, 1, "Membership should exist");

        // Remove membership
        store
            .db()
            .query("DELETE member_of WHERE in = node:doc_remove AND out = node:coll_remove")
            .await?
            .check()?;

        // Verify membership is removed (count should be 0)
        let result = store
            .db()
            .query("SELECT count() FROM member_of WHERE in = node:doc_remove GROUP ALL")
            .await?;

        let mut result = result.check()?;
        let count_result: Option<serde_json::Value> = result.take(0)?;
        // When no rows match GROUP ALL, the result is empty/none
        let count = count_result.and_then(|v| v["count"].as_u64()).unwrap_or(0);
        assert_eq!(count, 0, "Membership should be removed");

        Ok(())
    }

    #[tokio::test]
    async fn test_nested_collections_via_member_of() -> Result<()> {
        // Issue #808: Collection hierarchy uses member_of edges, not has_child
        let (store, _temp_dir) = create_test_db().await?;

        // Create nested collection hierarchy: hr -> policy -> vacation
        store
            .db()
            .query(
                r#"
                CREATE node:coll_hr CONTENT {
                    node_type: 'collection',
                    content: 'HR',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };

                CREATE node:coll_policy CONTENT {
                    node_type: 'collection',
                    content: 'Policy',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };

                CREATE node:coll_vacation CONTENT {
                    node_type: 'collection',
                    content: 'Vacation',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };
                "#,
            )
            .await?
            .check()?;

        // Issue #808: Create hierarchy using member_of edges (child -> member_of -> parent)
        // policy is member_of hr, vacation is member_of policy
        store
            .db()
            .query(
                r#"
                RELATE node:coll_policy->relationship->node:coll_hr CONTENT {
                    relationship_type: 'member_of',
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now(),
                    version: 1
                };

                RELATE node:coll_vacation->relationship->node:coll_policy CONTENT {
                    relationship_type: 'member_of',
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now(),
                    version: 1
                };
                "#,
            )
            .await?
            .check()?;

        // Verify hierarchy: policy is member_of hr
        let result = store
            .db()
            .query("SELECT VALUE out.content FROM relationship WHERE in = node:coll_policy AND relationship_type = 'member_of'")
            .await?;

        let mut result = result.check()?;
        let parents: Vec<String> = result.take(0)?;
        assert_eq!(parents.len(), 1, "Policy should have 1 parent (HR)");
        assert_eq!(parents[0], "HR");

        // Verify hierarchy: vacation is member_of policy
        let result = store
            .db()
            .query("SELECT VALUE out.content FROM relationship WHERE in = node:coll_vacation AND relationship_type = 'member_of'")
            .await?;

        let mut result = result.check()?;
        let parents: Vec<String> = result.take(0)?;
        assert_eq!(parents.len(), 1, "Vacation should have 1 parent (Policy)");
        assert_eq!(parents[0], "Policy");

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_with_multiple_node_types() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create a collection
        store
            .db()
            .query(
                r#"
                CREATE node:coll_mixed CONTENT {
                    node_type: 'collection',
                    content: 'Mixed Content Collection',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };
                "#,
            )
            .await?
            .check()?;

        // Create nodes of different types
        let uuid = "550e8400-e29b-41d4-a716-446655440100";
        store
            .db()
            .query(format!(
                r#"
                -- Text node
                CREATE node:text_node CONTENT {{
                    node_type: 'text',
                    content: 'Text document',
                    data: NONE,
                    version: 1,
                    properties: {{}},
                    created_at: time::now(),
                    modified_at: time::now()
                }};

                -- Task node (Universal Graph Architecture)
                CREATE node:`{uuid}` CONTENT {{
                    node_type: 'task',
                    content: 'Task item',
                    version: 1,
                    properties: {{ status: 'open', priority: 'high' }},
                    created_at: time::now(),
                    modified_at: time::now()
                }};
                "#
            ))
            .await?
            .check()?;

        // Add both to collection
        store
            .db()
            .query(format!(
                r#"
                RELATE node:text_node->member_of->node:coll_mixed CONTENT {{
                    created_at: time::now()
                }};
                RELATE node:`{uuid}`->member_of->node:coll_mixed CONTENT {{
                    created_at: time::now()
                }};
                "#
            ))
            .await?
            .check()?;

        // Query collection members with their types
        let result = store
            .db()
            .query("SELECT in.node_type AS node_type FROM member_of WHERE out = node:coll_mixed")
            .await?;

        let mut result = result.check()?;
        let members: Vec<serde_json::Value> = result.take(0)?;
        assert_eq!(
            members.len(),
            2,
            "Collection should have 2 members of different types"
        );

        // Verify we have both types
        let types: Vec<&str> = members
            .iter()
            .filter_map(|m| m["node_type"].as_str())
            .collect();
        assert!(types.contains(&"text"), "Should contain text node");
        assert!(types.contains(&"task"), "Should contain task node");

        Ok(())
    }

    #[tokio::test]
    async fn test_membership_idempotency() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create collection and document
        store
            .db()
            .query(
                r#"
                CREATE node:coll_idem CONTENT {
                    node_type: 'collection',
                    content: 'Idempotency Test',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };

                CREATE node:doc_idem CONTENT {
                    node_type: 'text',
                    content: 'Test Document',
                    data: NONE,
                    version: 1,
                    properties: {},
                    created_at: time::now(),
                    modified_at: time::now()
                };
                "#,
            )
            .await?
            .check()?;

        // Add membership twice - should not create duplicate edges
        // Using IF NOT EXISTS pattern
        store
            .db()
            .query(
                r#"
                LET $existing = SELECT * FROM member_of WHERE in = node:doc_idem AND out = node:coll_idem;
                IF array::len($existing) == 0 THEN {
                    RELATE node:doc_idem->member_of->node:coll_idem CONTENT {
                        created_at: time::now()
                    }
                } END;
                "#,
            )
            .await?
            .check()?;

        // Try to add again
        store
            .db()
            .query(
                r#"
                LET $existing = SELECT * FROM member_of WHERE in = node:doc_idem AND out = node:coll_idem;
                IF array::len($existing) == 0 THEN {
                    RELATE node:doc_idem->member_of->node:coll_idem CONTENT {
                        created_at: time::now()
                    }
                } END;
                "#,
            )
            .await?
            .check()?;

        // Verify only one edge exists (count)
        let result = store
            .db()
            .query("SELECT count() FROM member_of WHERE in = node:doc_idem AND out = node:coll_idem GROUP ALL")
            .await?;

        let mut result = result.check()?;
        let count_result: Option<serde_json::Value> = result.take(0)?;
        let count = count_result.and_then(|v| v["count"].as_u64()).unwrap_or(0);
        assert_eq!(count, 1, "Should have exactly one membership edge");

        Ok(())
    }
}

/// Tests for CollectionService async methods
/// These test the high-level service API that wraps the database operations
#[cfg(test)]
mod collection_service_tests {
    use anyhow::Result;
    use nodespace_core::db::SurrealStore;
    use nodespace_core::services::{CollectionService, NodeService};
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper to create test database with SurrealStore and NodeService
    /// Issue #813: CollectionService now requires both store and node_service
    async fn create_test_services() -> Result<(Arc<SurrealStore>, NodeService, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let mut store = Arc::new(SurrealStore::new(db_path).await?);
        let node_service = NodeService::new(&mut store).await?;
        Ok((store, node_service, temp_dir))
    }

    /// Helper to create a text node via raw SQL
    async fn create_text_node(store: &SurrealStore, id: &str, content: &str) -> Result<()> {
        store
            .db()
            .query(format!(
                r#"
                CREATE node:`{id}` CONTENT {{
                    node_type: 'text',
                    content: '{content}',
                    data: NONE,
                    version: 1,
                    properties: {{}},
                    created_at: time::now(),
                    modified_at: time::now()
                }};
                "#
            ))
            .await?
            .check()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_path_creates_collections() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Resolve a multi-level path - should create all collections
        let resolved = collection_service
            .resolve_path("hr:policy:vacation")
            .await?;

        // Should have created 3 collections
        assert_eq!(
            resolved.leaf_id().len(),
            36,
            "Should return a UUID for leaf"
        );

        // Verify all collections were created by checking they exist
        let hr_coll = collection_service.get_collection_by_name("hr").await?;
        assert!(hr_coll.is_some(), "HR collection should exist");
        assert_eq!(hr_coll.unwrap().node_type, "collection");

        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_path_reuses_existing() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Resolve path twice
        let first = collection_service.resolve_path("engineering").await?;
        let second = collection_service.resolve_path("engineering").await?;

        // Should return the same collection ID
        assert_eq!(
            first.leaf_id(),
            second.leaf_id(),
            "Should reuse existing collection"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_add_to_collection_by_path() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create a text node via raw SQL
        let node_id = "test-doc-1";
        create_text_node(&store, node_id, "Test document").await?;

        // Add to collection by path
        collection_service
            .add_to_collection_by_path(node_id, "projects:active")
            .await?;

        // Verify membership
        let collections = collection_service.get_node_collections(node_id).await?;
        assert_eq!(collections.len(), 1, "Node should belong to 1 collection");

        Ok(())
    }

    #[tokio::test]
    async fn test_add_to_collection_by_id() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create a collection first
        let resolved = collection_service.resolve_path("team").await?;
        let collection_id = resolved.leaf_id().to_string();

        // Create a text node
        let node_id = "team-doc-1";
        create_text_node(&store, node_id, "Team document").await?;

        // Add to collection by ID
        collection_service
            .add_to_collection(node_id, &collection_id)
            .await?;

        // Verify membership
        let collections = collection_service.get_node_collections(node_id).await?;
        assert_eq!(collections.len(), 1, "Node should belong to 1 collection");
        assert_eq!(collections[0], collection_id);

        Ok(())
    }

    #[tokio::test]
    async fn test_remove_from_collection() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create node
        let node_id = "temp-doc-1";
        create_text_node(&store, node_id, "Temporary doc").await?;

        // Add to collection
        collection_service
            .add_to_collection_by_path(node_id, "temp")
            .await?;

        // Get collection ID
        let collections = collection_service.get_node_collections(node_id).await?;
        assert_eq!(collections.len(), 1);
        let collection_id = collections[0].clone();

        // Remove from collection
        collection_service
            .remove_from_collection(node_id, &collection_id)
            .await?;

        // Verify removal
        let collections_after = collection_service.get_node_collections(node_id).await?;
        assert_eq!(
            collections_after.len(),
            0,
            "Node should not belong to any collection"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_get_collection_members() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create collection
        let resolved = collection_service.resolve_path("docs").await?;
        let collection_id = resolved.leaf_id().to_string();

        // Create multiple nodes
        create_text_node(&store, "doc-1", "Doc 1").await?;
        create_text_node(&store, "doc-2", "Doc 2").await?;
        create_text_node(&store, "doc-3", "Doc 3 (not in collection)").await?;

        // Add first two to collection
        collection_service
            .add_to_collection("doc-1", &collection_id)
            .await?;
        collection_service
            .add_to_collection("doc-2", &collection_id)
            .await?;
        // doc-3 not added

        // Get collection members
        let members = collection_service
            .get_collection_members(&collection_id)
            .await?;
        assert_eq!(members.len(), 2, "Collection should have 2 members");
        let member_ids: Vec<_> = members.iter().map(|n| n.id.as_str()).collect();
        assert!(member_ids.contains(&"doc-1"));
        assert!(member_ids.contains(&"doc-2"));
        assert!(!member_ids.contains(&"doc-3"));

        Ok(())
    }

    #[tokio::test]
    async fn test_node_multiple_collections() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create a node
        let node_id = "multi-coll-doc";
        create_text_node(&store, node_id, "Multi-collection doc").await?;

        // Add to multiple collections
        collection_service
            .add_to_collection_by_path(node_id, "hr")
            .await?;
        collection_service
            .add_to_collection_by_path(node_id, "legal")
            .await?;
        collection_service
            .add_to_collection_by_path(node_id, "compliance")
            .await?;

        // Verify memberships
        let collections = collection_service.get_node_collections(node_id).await?;
        assert_eq!(collections.len(), 3, "Node should belong to 3 collections");

        Ok(())
    }

    #[tokio::test]
    async fn test_find_collection_by_path() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // First verify collection doesn't exist
        let not_found = collection_service
            .find_collection_by_path("nonexistent")
            .await?;
        assert!(not_found.is_none(), "Collection should not exist yet");

        // Create the collection
        collection_service.resolve_path("newcoll").await?;

        // Now it should be found
        let found = collection_service
            .find_collection_by_path("newcoll")
            .await?;
        assert!(found.is_some(), "Collection should now exist");

        Ok(())
    }

    #[tokio::test]
    async fn test_add_to_collection_idempotent() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create node
        let node_id = "idem-doc";
        create_text_node(&store, node_id, "Doc").await?;

        // Add to same collection multiple times
        collection_service
            .add_to_collection_by_path(node_id, "archive")
            .await?;
        collection_service
            .add_to_collection_by_path(node_id, "archive")
            .await?;
        collection_service
            .add_to_collection_by_path(node_id, "archive")
            .await?;

        // Should still only have one membership
        let collections = collection_service.get_node_collections(node_id).await?;
        assert_eq!(
            collections.len(),
            1,
            "Should have exactly one membership despite multiple adds"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_nested_path_creates_hierarchy() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Resolve a 3-level path
        let resolved = collection_service.resolve_path("company:dept:team").await?;

        // Verify all collections were created
        let company = collection_service.get_collection_by_name("company").await?;
        assert!(company.is_some(), "Company collection should exist");
        assert_eq!(company.as_ref().unwrap().node_type, "collection");

        let dept = collection_service.get_collection_by_name("dept").await?;
        assert!(dept.is_some(), "Dept collection should exist");
        assert_eq!(dept.as_ref().unwrap().node_type, "collection");

        let team = collection_service.get_collection_by_name("team").await?;
        assert!(team.is_some(), "Team collection should exist");
        assert_eq!(team.as_ref().unwrap().node_type, "collection");
        assert_eq!(team.as_ref().unwrap().content, "team");

        // Verify the resolved path points to the team (leaf) collection
        assert_eq!(resolved.leaf_id(), team.as_ref().unwrap().id.as_str());

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_with_special_characters() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create collection with spaces and special chars (but not colon)
        let _resolved = collection_service
            .resolve_path("Q4 Planning (2024)")
            .await?;

        // Verify it was created
        let coll = collection_service
            .get_collection_by_name("Q4 Planning (2024)")
            .await?;
        assert!(coll.is_some(), "Collection with special chars should exist");

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_name_uniqueness_enforced() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create first collection
        let resolved = collection_service.resolve_path("unique-test").await?;
        let first_id = resolved.leaf_id().to_string();

        // Try to create another collection with the same name via resolve_path
        // This should reuse the existing one, not create a new one
        let resolved_again = collection_service.resolve_path("unique-test").await?;
        assert_eq!(
            resolved_again.leaf_id(),
            first_id,
            "resolve_path should reuse existing collection"
        );

        // Also verify that creating a collection node directly with duplicate name fails
        use nodespace_core::models::Node;
        let duplicate_node = Node::new(
            "collection".to_string(),
            "unique-test".to_string(),
            serde_json::json!({}),
        );
        let result = store.create_node(duplicate_node, None).await;
        assert!(result.is_err(), "Creating duplicate collection should fail");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("already exists"),
            "Error should mention collection already exists: {}",
            err_msg
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_uniqueness_case_insensitive() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create collection with lowercase name
        collection_service.resolve_path("engineering").await?;

        // Try to create with different case - should fail
        use nodespace_core::models::Node;
        let uppercase_node = Node::new(
            "collection".to_string(),
            "ENGINEERING".to_string(),
            serde_json::json!({}),
        );
        let result = store.create_node(uppercase_node, None).await;
        assert!(
            result.is_err(),
            "Creating collection with same name (different case) should fail"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_multi_path_same_leaf() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Scenario: "berlin" can be reached via multiple paths
        // hr:policy:vacation:berlin - creates hr, policy, vacation, berlin
        let resolved1 = collection_service
            .resolve_path("hr:policy:vacation:berlin")
            .await?;
        let berlin_id = resolved1.leaf_id().to_string();

        // engineering:office:berlin - creates engineering, office, reuses berlin
        let resolved2 = collection_service
            .resolve_path("engineering:office:berlin")
            .await?;

        // Both paths should resolve to the SAME berlin collection
        assert_eq!(
            resolved2.leaf_id(),
            berlin_id,
            "Both paths should resolve to the same 'berlin' collection"
        );

        // Verify all collections were created (flat, no hierarchy)
        assert!(collection_service
            .get_collection_by_name("hr")
            .await?
            .is_some());
        assert!(collection_service
            .get_collection_by_name("policy")
            .await?
            .is_some());
        assert!(collection_service
            .get_collection_by_name("vacation")
            .await?
            .is_some());
        assert!(collection_service
            .get_collection_by_name("engineering")
            .await?
            .is_some());
        assert!(collection_service
            .get_collection_by_name("office")
            .await?
            .is_some());
        assert!(collection_service
            .get_collection_by_name("berlin")
            .await?
            .is_some());

        // Add a node to berlin via one path
        create_text_node(&store, "berlin-doc", "Berlin office document").await?;
        collection_service
            .add_to_collection("berlin-doc", &berlin_id)
            .await?;

        // Verify the node is in berlin (reachable via either path conceptually)
        let members = collection_service
            .get_collection_members(&berlin_id)
            .await?;
        let member_ids: Vec<_> = members.iter().map(|n| n.id.as_str()).collect();
        assert!(
            member_ids.contains(&"berlin-doc"),
            "Berlin collection should contain the document"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_hierarchy_via_member_of() -> Result<()> {
        // Issue #808: Collection path resolution creates hierarchy between collections
        // using member_of edges, not has_child edges.
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create collections via path
        collection_service.resolve_path("parent:child").await?;

        // Get both collections
        let parent = collection_service
            .get_collection_by_name("parent")
            .await?
            .unwrap();
        let child = collection_service
            .get_collection_by_name("child")
            .await?
            .unwrap();

        // Verify child has no has_child parent (collections don't use has_child for hierarchy)
        let child_parent = store.get_parent(&child.id).await?;
        assert!(
            child_parent.is_none(),
            "Collections should NOT use has_child - 'child' should have no has_child parent"
        );

        // Verify parent has no has_child children
        let parent_children = store.get_children(Some(&parent.id)).await?;
        assert!(
            parent_children.is_empty(),
            "Collections should NOT use has_child - 'parent' should have no has_child children"
        );

        // Issue #808: Verify hierarchy exists via member_of edges
        // child should be a member_of parent
        let child_memberships = collection_service.get_node_collections(&child.id).await?;
        assert_eq!(
            child_memberships.len(),
            1,
            "Child collection should belong to 1 parent collection via member_of"
        );
        assert_eq!(
            child_memberships[0], parent.id,
            "Child should be member_of parent collection"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_hierarchy_three_levels() -> Result<()> {
        // Issue #808: Test deeper hierarchy creation
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create a 3-level path
        collection_service
            .resolve_path("grandparent:parent:child")
            .await?;

        // Get all collections
        let grandparent = collection_service
            .get_collection_by_name("grandparent")
            .await?
            .unwrap();
        let parent = collection_service
            .get_collection_by_name("parent")
            .await?
            .unwrap();
        let child = collection_service
            .get_collection_by_name("child")
            .await?
            .unwrap();

        // Verify grandparent has no parent (root of hierarchy)
        let grandparent_memberships = collection_service
            .get_node_collections(&grandparent.id)
            .await?;
        assert!(
            grandparent_memberships.is_empty(),
            "Grandparent should have no parent collection"
        );

        // Verify parent is member_of grandparent
        let parent_memberships = collection_service.get_node_collections(&parent.id).await?;
        assert_eq!(parent_memberships.len(), 1);
        assert_eq!(parent_memberships[0], grandparent.id);

        // Verify child is member_of parent
        let child_memberships = collection_service.get_node_collections(&child.id).await?;
        assert_eq!(child_memberships.len(), 1);
        assert_eq!(child_memberships[0], parent.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_multi_parent_dag() -> Result<()> {
        // Issue #808: Collections can have multiple parents (DAG structure)
        // Example: "berlin" can be reached via multiple paths
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // First path: hr:policy:vacation:berlin
        collection_service
            .resolve_path("hr:policy:vacation:berlin")
            .await?;

        // Second path: engineering:offices:berlin (berlin already exists, adds second parent)
        collection_service
            .resolve_path("engineering:offices:berlin")
            .await?;

        // Get berlin collection
        let berlin = collection_service
            .get_collection_by_name("berlin")
            .await?
            .unwrap();

        // Verify berlin has TWO parents: vacation and offices
        let berlin_memberships = collection_service.get_node_collections(&berlin.id).await?;
        assert_eq!(
            berlin_memberships.len(),
            2,
            "Berlin should have 2 parent collections (DAG structure)"
        );

        // Get parent collections
        let vacation = collection_service
            .get_collection_by_name("vacation")
            .await?
            .unwrap();
        let offices = collection_service
            .get_collection_by_name("offices")
            .await?
            .unwrap();

        // Verify both parents are present
        assert!(
            berlin_memberships.contains(&vacation.id),
            "Berlin should be member_of vacation"
        );
        assert!(
            berlin_memberships.contains(&offices.id),
            "Berlin should be member_of offices"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_hierarchy_idempotent() -> Result<()> {
        // Issue #808: Resolving the same path multiple times should not create duplicate relationships
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Resolve the same path multiple times
        collection_service.resolve_path("a:b:c").await?;
        collection_service.resolve_path("a:b:c").await?;
        collection_service.resolve_path("a:b:c").await?;

        // Get collections
        let a = collection_service
            .get_collection_by_name("a")
            .await?
            .unwrap();
        let b = collection_service
            .get_collection_by_name("b")
            .await?
            .unwrap();
        let c = collection_service
            .get_collection_by_name("c")
            .await?
            .unwrap();

        // Verify each relationship exists exactly once
        let b_memberships = collection_service.get_node_collections(&b.id).await?;
        assert_eq!(
            b_memberships.len(),
            1,
            "b should have exactly 1 parent despite multiple resolve_path calls"
        );
        assert_eq!(b_memberships[0], a.id);

        let c_memberships = collection_service.get_node_collections(&c.id).await?;
        assert_eq!(
            c_memberships.len(),
            1,
            "c should have exactly 1 parent despite multiple resolve_path calls"
        );
        assert_eq!(c_memberships[0], b.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_member_of_only_allows_collection_targets() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create a text node (not a collection)
        create_text_node(&store, "text-node", "Just a text node").await?;

        // Create another text node to try to add as member
        create_text_node(&store, "doc-node", "Document").await?;

        // Try to add doc-node as member of text-node (should fail)
        // Validation is in service layer, not store layer
        let result = collection_service
            .add_to_collection("doc-node", "text-node")
            .await;
        assert!(
            result.is_err(),
            "Should not be able to add member to non-collection node"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("non-collection") || err_msg.contains("type 'text'"),
            "Error should mention non-collection: {}",
            err_msg
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_member_of_nonexistent_collection_fails() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create a text node
        create_text_node(&store, "doc-node", "Document").await?;

        // Try to add to non-existent collection
        // Validation is in service layer, not store layer
        let result = collection_service
            .add_to_collection("doc-node", "nonexistent-collection")
            .await;
        assert!(
            result.is_err(),
            "Should not be able to add member to nonexistent collection"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not found"),
            "Error should mention collection not found: {}",
            err_msg
        );

        Ok(())
    }

    /// Test that collection membership operations emit unified relationship events
    ///
    /// **Issue #813**: CollectionService now delegates to NodeService for event emission.
    /// This test verifies that events are properly emitted via NodeService.
    #[tokio::test]
    async fn test_collection_membership_events_emitted() -> Result<()> {
        use nodespace_core::db::events::DomainEvent;
        use tokio::time::{timeout, Duration};

        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Subscribe to NodeService events (Issue #813: events come from NodeService)
        let mut event_rx = node_service.subscribe_to_events();

        // Create a collection
        let resolved = collection_service.resolve_path("events-test").await?;
        let collection_id = resolved.leaf_id().to_string();

        // Drain creation events from resolve_path
        while event_rx.try_recv().is_ok() {}

        // Create a text node via raw SQL (doesn't go through NodeService)
        create_text_node(&store, "event-doc", "Event Test Doc").await?;

        // Add node to collection - should emit RelationshipCreated via NodeService
        collection_service
            .add_to_collection("event-doc", &collection_id)
            .await?;

        // Check for RelationshipCreated event (unified format from NodeService)
        let event = timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");
        match event {
            DomainEvent::RelationshipCreated {
                relationship,
                source_client_id: _,
            } => {
                assert_eq!(relationship.from_id, "event-doc");
                assert_eq!(relationship.to_id, collection_id);
                assert_eq!(relationship.relationship_type, "member_of");
            }
            other => panic!("Expected RelationshipCreated, got {:?}", other),
        }

        // Remove node from collection - should emit RelationshipDeleted via NodeService
        collection_service
            .remove_from_collection("event-doc", &collection_id)
            .await?;

        // Check for RelationshipDeleted event (unified format from NodeService)
        let event = timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");
        match event {
            DomainEvent::RelationshipDeleted {
                relationship_type,
                source_client_id: _,
                ..
            } => {
                assert_eq!(relationship_type, "member_of");
            }
            other => panic!("Expected RelationshipDeleted, got {:?}", other),
        }

        Ok(())
    }

    /// Test that collection membership operations work correctly end-to-end
    ///
    /// Issue #813: This test verifies basic add/remove membership operations
    /// work through the CollectionService -> NodeService delegation pattern.
    #[tokio::test]
    async fn test_collection_membership_add_remove_operations() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create a collection
        let resolved = collection_service.resolve_path("operations-test").await?;
        let collection_id = resolved.leaf_id().to_string();

        // Create a text node
        create_text_node(&store, "ops-doc", "Operations Test Doc").await?;

        // Add node to collection
        collection_service
            .add_to_collection("ops-doc", &collection_id)
            .await?;

        // Verify membership was added
        let memberships = collection_service.get_node_collections("ops-doc").await?;
        assert_eq!(memberships.len(), 1, "Node should have one membership");
        assert!(
            memberships.iter().any(|id| id.contains(&collection_id)),
            "Node should be member of the created collection"
        );

        // Remove from collection
        collection_service
            .remove_from_collection("ops-doc", &collection_id)
            .await?;

        // Verify the membership was removed
        let memberships = collection_service.get_node_collections("ops-doc").await?;
        assert!(
            memberships.is_empty(),
            "Node should have no memberships after removal"
        );

        Ok(())
    }

    /// Test get_all_collections_with_member_counts (Issue #817)
    ///
    /// This test validates that the optimized single-query method for fetching
    /// all collections with their member counts works correctly.
    #[tokio::test]
    async fn test_get_all_collections_with_member_counts() -> Result<()> {
        let (store, node_service, _temp_dir) = create_test_services().await?;
        let collection_service = CollectionService::new(&store, &node_service);

        // Create multiple collections
        let resolved_a = collection_service.resolve_path("collection-a").await?;
        let collection_a_id = resolved_a.leaf_id().to_string();

        let resolved_b = collection_service.resolve_path("collection-b").await?;
        let collection_b_id = resolved_b.leaf_id().to_string();

        let resolved_empty = collection_service.resolve_path("empty-collection").await?;
        let _empty_collection_id = resolved_empty.leaf_id().to_string();

        // Create some text nodes
        create_text_node(&store, "doc-a1", "Document A1").await?;
        create_text_node(&store, "doc-a2", "Document A2").await?;
        create_text_node(&store, "doc-b1", "Document B1").await?;

        // Add nodes to collections:
        // - collection-a: 2 members (doc-a1, doc-a2)
        // - collection-b: 1 member (doc-b1)
        // - empty-collection: 0 members
        collection_service
            .add_to_collection("doc-a1", &collection_a_id)
            .await?;
        collection_service
            .add_to_collection("doc-a2", &collection_a_id)
            .await?;
        collection_service
            .add_to_collection("doc-b1", &collection_b_id)
            .await?;

        // Get all collections with member counts
        let collections_with_counts = collection_service.get_all_collections_with_counts().await?;

        // Should have 3 collections
        assert_eq!(
            collections_with_counts.len(),
            3,
            "Should have 3 collections"
        );

        // Verify member counts
        let find_count = |name: &str| -> Option<usize> {
            collections_with_counts
                .iter()
                .find(|(node, _)| node.content == name)
                .map(|(_, count)| *count)
        };

        assert_eq!(
            find_count("collection-a"),
            Some(2),
            "collection-a should have 2 members"
        );
        assert_eq!(
            find_count("collection-b"),
            Some(1),
            "collection-b should have 1 member"
        );
        assert_eq!(
            find_count("empty-collection"),
            Some(0),
            "empty-collection should have 0 members"
        );

        Ok(())
    }
}
