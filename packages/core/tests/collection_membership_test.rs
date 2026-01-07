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
    async fn test_nested_collections() -> Result<()> {
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

        // Create parent-child relationships using has_child edges
        // This establishes the collection hierarchy
        store
            .db()
            .query(
                r#"
                RELATE node:coll_hr->has_child->node:coll_policy CONTENT {
                    order: 1.0,
                    created_at: time::now(),
                    version: 1
                };

                RELATE node:coll_policy->has_child->node:coll_vacation CONTENT {
                    order: 1.0,
                    created_at: time::now(),
                    version: 1
                };
                "#,
            )
            .await?
            .check()?;

        // Verify hierarchy by traversing from root
        let result = store
            .db()
            .query("SELECT out.content FROM has_child WHERE in = node:coll_hr")
            .await?;

        let mut result = result.check()?;
        let children: Vec<serde_json::Value> = result.take(0)?;
        assert_eq!(children.len(), 1, "HR should have 1 child (policy)");

        // Verify policy -> vacation link
        let result = store
            .db()
            .query("SELECT out.content FROM has_child WHERE in = node:coll_policy")
            .await?;

        let mut result = result.check()?;
        let grandchildren: Vec<serde_json::Value> = result.take(0)?;
        assert_eq!(
            grandchildren.len(),
            1,
            "Policy should have 1 child (vacation)"
        );

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
    use nodespace_core::services::CollectionService;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper to create test database with SurrealStore
    async fn create_test_store() -> Result<(Arc<SurrealStore>, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SurrealStore::new(db_path).await?);
        Ok((store, temp_dir))
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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        assert!(members.contains(&"doc-1".to_string()));
        assert!(members.contains(&"doc-2".to_string()));
        assert!(!members.contains(&"doc-3".to_string()));

        Ok(())
    }

    #[tokio::test]
    async fn test_node_multiple_collections() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        assert!(
            members.contains(&"berlin-doc".to_string()),
            "Berlin collection should contain the document"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_collections_are_flat_no_hierarchy() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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

        // Verify child has no parent (collections are flat)
        let child_parent = store.get_parent(&child.id).await?;
        assert!(
            child_parent.is_none(),
            "Collections should be flat - 'child' should have no parent node"
        );

        // Verify parent has no children (in the has_child sense)
        let parent_children = store.get_children(Some(&parent.id)).await?;
        assert!(
            parent_children.is_empty(),
            "Collections should be flat - 'parent' should have no child nodes"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_member_of_only_allows_collection_targets() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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
        let (store, _temp_dir) = create_test_store().await?;
        let collection_service = CollectionService::new(&store);

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

    #[tokio::test]
    async fn test_collection_membership_events_emitted() -> Result<()> {
        use nodespace_core::db::events::DomainEvent;
        use tokio::sync::broadcast;

        let (store, _temp_dir) = create_test_store().await?;

        // Create broadcast channel for events
        let (event_tx, mut event_rx) = broadcast::channel::<DomainEvent>(16);

        // Create service with event emission
        let collection_service =
            CollectionService::with_events(&store, event_tx, Some("test-client".to_string()));

        // Create a collection
        let resolved = collection_service.resolve_path("events-test").await?;
        let collection_id = resolved.leaf_id().to_string();

        // Create a text node
        create_text_node(&store, "event-doc", "Event Test Doc").await?;

        // Add node to collection - should emit CollectionMemberAdded
        collection_service
            .add_to_collection("event-doc", &collection_id)
            .await?;

        // Check for CollectionMemberAdded event
        let event = event_rx.try_recv()?;
        match event {
            DomainEvent::CollectionMemberAdded {
                membership,
                source_client_id,
            } => {
                assert_eq!(membership.member_id, "event-doc");
                assert_eq!(membership.collection_id, collection_id);
                assert_eq!(source_client_id, Some("test-client".to_string()));
            }
            other => panic!("Expected CollectionMemberAdded, got {:?}", other),
        }

        // Remove node from collection - should emit CollectionMemberRemoved
        collection_service
            .remove_from_collection("event-doc", &collection_id)
            .await?;

        // Check for CollectionMemberRemoved event
        let event = event_rx.try_recv()?;
        match event {
            DomainEvent::CollectionMemberRemoved {
                membership,
                source_client_id,
            } => {
                assert_eq!(membership.member_id, "event-doc");
                assert_eq!(membership.collection_id, collection_id);
                assert_eq!(source_client_id, Some("test-client".to_string()));
            }
            other => panic!("Expected CollectionMemberRemoved, got {:?}", other),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_service_without_events_does_not_fail() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create service WITHOUT event emission (default constructor)
        let collection_service = CollectionService::new(&store);

        // Create a collection
        let resolved = collection_service.resolve_path("no-events-test").await?;
        let collection_id = resolved.leaf_id().to_string();

        // Create a text node
        create_text_node(&store, "no-event-doc", "No Event Test Doc").await?;

        // Operations should succeed without event emission configured
        collection_service
            .add_to_collection("no-event-doc", &collection_id)
            .await?;

        collection_service
            .remove_from_collection("no-event-doc", &collection_id)
            .await?;

        // Verify the membership was removed (operations worked correctly)
        let memberships = collection_service
            .get_node_collections("no-event-doc")
            .await?;
        assert!(
            memberships.is_empty(),
            "Node should have no memberships after removal"
        );

        Ok(())
    }
}
