//! The one participation rule, at every surface (ADR-087 §2).
//!
//! An archived node participates in nothing: queries and counts, keyword and
//! semantic search, lists (roots, collection members, saved queries,
//! backlinks, related nodes), the `@` picker, the agent's workspace context
//! and the play engine. Each surface is tested the same way: the archived
//! node is absent by default and present with `include_archived`, where the
//! surface has that opt-in. A read by id always returns the node.
//!
//! The rule itself is unit-tested in `governance`; these tests are that it is
//! applied.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, NodeFilter, NodeQuery, NodeUpdate};
use nodespace_core::ops::rel_ops::{get_node_relationships, get_related_nodes, GetRelatedInput};
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::{
    CollectionService, CreateNodeParams, InsertPositionOwned, NodeService, QueryDefinition,
    QueryService,
};
use nodespace_core::PlaybookEngine;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::watch;
use tokio::time::timeout;

async fn test_service() -> (Arc<NodeService>, TempDir) {
    let temp_dir = TempDir::new().expect("tempdir creation failed");
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(
        SqliteStore::new(db_path)
            .await
            .expect("SqliteStore init failed"),
    );
    let service = Arc::new(
        NodeService::new(&mut store)
            .await
            .expect("NodeService init failed"),
    );
    (service, temp_dir)
}

async fn create(
    svc: &Arc<NodeService>,
    node_type: &str,
    content: &str,
    properties: serde_json::Value,
) -> String {
    svc.create_node(Node::new(
        node_type.to_string(),
        content.to_string(),
        properties,
    ))
    .await
    .unwrap_or_else(|e| panic!("creating {node_type} '{content}' failed: {e}"))
}

async fn create_under(svc: &Arc<NodeService>, parent_id: &str, content: &str) -> String {
    svc.create_node_with_parent(CreateNodeParams {
        id: None,
        node_type: "text".to_string(),
        content: content.to_string(),
        parent_id: Some(parent_id.to_string()),
        position: InsertPositionOwned::End,
        properties: json!({}),
        lifecycle_status: None,
    })
    .await
    .expect("creating a child failed")
}

/// Archive or unarchive through the generic update: the one write path
/// (ADR-087 §4).
async fn set_lifecycle(svc: &Arc<NodeService>, id: &str, status: &str) {
    let version = svc
        .get_node(id)
        .await
        .unwrap()
        .expect("the node exists")
        .version;
    svc.update_node(
        id,
        version,
        NodeUpdate::new().with_lifecycle_status(status.to_string()),
    )
    .await
    .unwrap_or_else(|e| panic!("setting lifecycle to {status} failed: {e}"));
}

async fn archive(svc: &Arc<NodeService>, id: &str) {
    set_lifecycle(svc, id, "archived").await;
}

async fn unarchive(svc: &Arc<NodeService>, id: &str) {
    set_lifecycle(svc, id, "active").await;
}

fn has(nodes: &[Node], id: &str) -> bool {
    nodes.iter().any(|n| n.id == id)
}

// ---------------------------------------------------------------------------
// Queries and counts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_archived_node_is_in_no_query_or_count() {
    let (svc, _tmp) = test_service().await;
    let live = create(&svc, "text", "Harbour survey live", json!({})).await;
    let gone = create(&svc, "text", "Harbour survey retired", json!({})).await;
    archive(&svc, &gone).await;

    // By type.
    let by_type = |include_archived| {
        NodeFilter::new()
            .with_node_type("text".to_string())
            .with_include_archived(include_archived)
    };
    let default = svc.query_nodes(by_type(false)).await.unwrap();
    assert!(has(&default, &live));
    assert!(!has(&default, &gone), "archived is absent by default");
    let opted_in = svc.query_nodes(by_type(true)).await.unwrap();
    assert!(has(&opted_in, &live));
    assert!(has(&opted_in, &gone), "include_archived returns it");

    // The convenience form the engine and the agent's seed reads use.
    assert!(!has(
        &svc.query_nodes_by_type("text", false).await.unwrap(),
        &gone
    ));
    assert!(has(
        &svc.query_nodes_by_type("text", true).await.unwrap(),
        &gone
    ));

    // Counts agree with the queries.
    let count = |include_archived| {
        let svc = svc.clone();
        async move {
            svc.count_nodes(NodeQuery {
                node_type: Some("text".to_string()),
                include_archived,
                ..Default::default()
            })
            .await
            .unwrap()
        }
    };
    assert_eq!(count(false).await, default.len() as i64);
    assert_eq!(count(true).await, count(false).await + 1);

    // Scoped to an explicit id set, as a collection view is.
    let scoped = |include_archived| {
        NodeFilter::new()
            .with_ids(vec![live.clone(), gone.clone()])
            .with_include_archived(include_archived)
    };
    let ids = svc.query_nodes(scoped(false)).await.unwrap();
    assert!(has(&ids, &live) && !has(&ids, &gone));
    assert!(has(&svc.query_nodes(scoped(true)).await.unwrap(), &gone));

    // A read by id returns it: archived is hidden, not gone, and not read-only.
    let read = svc.get_node(&gone).await.unwrap().expect("read by id");
    assert_eq!(read.content, "Harbour survey retired");
    svc.update_node(
        &gone,
        read.version,
        NodeUpdate::new().with_content("Harbour survey retired, edited".to_string()),
    )
    .await
    .expect("an archived node is edited like any node");
}

#[tokio::test]
async fn keyword_search_leaves_an_archived_node_out() {
    let (svc, _tmp) = test_service().await;
    let live = create(&svc, "text", "Lighthouse rota current", json!({})).await;
    let gone = create(&svc, "text", "Lighthouse rota superseded", json!({})).await;
    archive(&svc, &gone).await;

    for by_title in [true, false] {
        let query = |include_archived| {
            let mut q = NodeQuery {
                include_archived,
                ..Default::default()
            };
            if by_title {
                q.title_contains = Some("Lighthouse rota".to_string());
            } else {
                q.content_contains = Some("Lighthouse rota".to_string());
            }
            q
        };
        let default = svc.query_nodes_simple(query(false)).await.unwrap();
        assert!(has(&default, &live), "by_title={by_title}");
        assert!(!has(&default, &gone), "by_title={by_title}");
        let opted_in = svc.query_nodes_simple(query(true)).await.unwrap();
        assert!(has(&opted_in, &gone), "by_title={by_title}");
    }
}

/// The title search's stem fallback is a second statement; it applies the
/// same rule as the exact match it stands in for.
#[tokio::test]
async fn the_title_stem_fallback_leaves_an_archived_node_out() {
    let (svc, _tmp) = test_service().await;
    let gone = create(&svc, "text", "Grocery store errands", json!({})).await;
    archive(&svc, &gone).await;

    let query = |include_archived| NodeQuery {
        title_contains: Some("groceries".to_string()),
        include_archived,
        ..Default::default()
    };
    let default = svc.query_nodes_simple(query(false)).await.unwrap();
    assert!(!has(&default, &gone));
    let opted_in = svc.query_nodes_simple(query(true)).await.unwrap();
    assert!(has(&opted_in, &gone), "the fallback finds it on opt-in");
}

#[tokio::test]
async fn a_saved_query_leaves_an_archived_node_out() {
    let (svc, _tmp) = test_service().await;
    let live = create(&svc, "text", "Ledger entry kept", json!({})).await;
    let gone = create(&svc, "text", "Ledger entry retired", json!({})).await;
    archive(&svc, &gone).await;

    let queries = QueryService::new(svc.store().clone());
    for target_type in ["text", "*"] {
        let definition = QueryDefinition {
            target_type: target_type.to_string(),
            filters: vec![],
            sorting: None,
            limit: None,
        };
        let rows = queries.execute(&definition).await.unwrap();
        assert!(has(&rows, &live), "target_type={target_type}");
        assert!(!has(&rows, &gone), "target_type={target_type}");
        assert_eq!(
            queries.count(&definition).await.unwrap(),
            rows.len() as i64,
            "the count is of the same rows (target_type={target_type})"
        );
    }
}

// ---------------------------------------------------------------------------
// Lists
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_root_list_leaves_an_archived_root_out() {
    let (svc, _tmp) = test_service().await;
    let live = create(&svc, "text", "A live page", json!({})).await;
    let gone = create(&svc, "text", "A retired page", json!({})).await;
    archive(&svc, &gone).await;

    let default = svc.get_roots(None, None, false).await.unwrap();
    assert!(has(&default, &live) && !has(&default, &gone));
    let opted_in = svc.get_roots(None, None, true).await.unwrap();
    assert!(has(&opted_in, &gone));

    assert_eq!(svc.count_roots(false).await.unwrap(), default.len() as i64);
    assert_eq!(svc.count_roots(true).await.unwrap(), opted_in.len() as i64);
}

#[tokio::test]
async fn a_collection_lists_and_counts_only_participating_members() {
    let (svc, _tmp) = test_service().await;
    let collection = create(&svc, "collection", "Fieldwork", json!({})).await;
    let live = create(&svc, "text", "Site notes", json!({})).await;
    let gone = create(&svc, "text", "Old site notes", json!({})).await;
    for member in [&live, &gone] {
        svc.create_relationship(member, "member_of", &collection, json!({}))
            .await
            .unwrap();
    }
    archive(&svc, &gone).await;

    let collections = CollectionService::new(svc.store(), &svc);

    let members = collections
        .get_collection_members(&collection, false)
        .await
        .unwrap();
    assert!(has(&members, &live) && !has(&members, &gone));
    let all = collections
        .get_collection_members(&collection, true)
        .await
        .unwrap();
    assert!(has(&all, &gone), "include_archived returns the member");

    let recursive = collections
        .get_collection_members_recursive(&collection, false)
        .await
        .unwrap();
    assert!(recursive.contains(&live) && !recursive.contains(&gone));
    assert!(collections
        .get_collection_members_recursive(&collection, true)
        .await
        .unwrap()
        .contains(&gone));

    // The sidebar's badge counts what the list shows.
    let counted = collections.get_all_collections_with_counts().await.unwrap();
    let (_, count, _) = counted
        .iter()
        .find(|(node, _, _)| node.id == collection)
        .expect("the collection is listed");
    assert_eq!(*count, 1);
}

#[tokio::test]
async fn an_archived_collection_is_in_no_collection_list() {
    let (svc, _tmp) = test_service().await;
    let parent = create(&svc, "collection", "Expeditions", json!({})).await;
    let sub = create(&svc, "collection", "Northern", json!({})).await;
    svc.create_relationship(&sub, "member_of", &parent, json!({}))
        .await
        .unwrap();
    let page = create(&svc, "text", "Northern route", json!({})).await;
    svc.create_relationship(&page, "member_of", &sub, json!({}))
        .await
        .unwrap();

    let collections = CollectionService::new(svc.store(), &svc);
    assert!(collections
        .get_collection_members_recursive(&parent, false)
        .await
        .unwrap()
        .contains(&page));

    archive(&svc, &sub).await;

    assert!(
        !collections
            .get_all_collection_descriptions()
            .await
            .unwrap()
            .iter()
            .any(|(name, _)| name == "Northern"),
        "an archived collection is not named"
    );
    assert!(collections
        .get_collection_by_name("Northern")
        .await
        .unwrap()
        .is_none());
    assert!(!collections
        .get_all_collections_with_counts()
        .await
        .unwrap()
        .iter()
        .any(|(node, _, _)| node.id == sub));

    // An archived sub-collection is not descended into.
    let reached = collections
        .get_collection_members_recursive(&parent, false)
        .await
        .unwrap();
    assert!(!reached.contains(&sub) && !reached.contains(&page));
    assert!(collections
        .get_collection_members_recursive(&parent, true)
        .await
        .unwrap()
        .contains(&page));
}

#[tokio::test]
async fn backlinks_leave_an_archived_node_out() {
    let (svc, _tmp) = test_service().await;
    let target = create(&svc, "text", "The referenced page", json!({})).await;
    let live = create(&svc, "text", "A live reference", json!({})).await;
    let gone = create(&svc, "text", "A retired reference", json!({})).await;
    for source in [&live, &gone] {
        svc.create_mention(source, &target).await.unwrap();
    }
    archive(&svc, &gone).await;

    let mentioning = |include_archived| NodeQuery {
        mentioned_by: Some(target.clone()),
        include_archived,
        ..Default::default()
    };
    let default = svc.query_nodes_simple(mentioning(false)).await.unwrap();
    assert!(has(&default, &live) && !has(&default, &gone));
    assert!(has(
        &svc.query_nodes_simple(mentioning(true)).await.unwrap(),
        &gone
    ));
    assert_eq!(
        svc.count_nodes(mentioning(false)).await.unwrap(),
        default.len() as i64
    );

    // The same list as ids.
    assert_eq!(
        svc.get_mentioned_by(&target).await.unwrap(),
        vec![live.clone()]
    );

    // And in the other direction: a node's own mentions leave out an archived
    // target, while the edge to it stays.
    archive(&svc, &target).await;
    assert!(svc.get_mentions(&live).await.unwrap().is_empty());
    assert_eq!(
        svc.store().get_outgoing_mentions(&live).await.unwrap(),
        vec![target.clone()]
    );
    unarchive(&svc, &target).await;
    assert_eq!(svc.get_mentions(&live).await.unwrap(), vec![target.clone()]);

    // The backlinks panel lists the pages the mentions sit in.
    let containers: Vec<String> = svc
        .get_mentioning_containers(&target)
        .await
        .unwrap()
        .into_iter()
        .map(|container| container.id)
        .collect();
    assert_eq!(containers, vec![live.clone()]);
}

#[tokio::test]
async fn related_nodes_leave_an_archived_node_out() {
    let (svc, _tmp) = test_service().await;
    let blocker = create(&svc, "task", "Order the timber", json!({})).await;
    let live = create(&svc, "task", "Frame the roof", json!({})).await;
    let gone = create(&svc, "task", "Frame the old shed", json!({})).await;
    for blocked in [&live, &gone] {
        svc.create_relationship(&blocker, "blocks", blocked, json!({}))
            .await
            .unwrap();
    }
    archive(&svc, &gone).await;

    let related = get_related_nodes(
        &svc,
        GetRelatedInput {
            node_id: blocker.clone(),
            relationship_name: "blocks".to_string(),
            direction: "out".to_string(),
        },
    )
    .await
    .unwrap();
    let ids: Vec<&str> = related
        .related_nodes
        .iter()
        .filter_map(|n| n.get("id").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(ids, vec![live.as_str()]);
    assert_eq!(related.count, 1);

    // The relationship viewer's groups list the same nodes.
    let groups = get_node_relationships(&svc, &blocker).await.unwrap();
    let blocks = groups
        .groups
        .iter()
        .find(|g| g.relationship_name == "blocks" && g.direction == "out")
        .expect("the declared group is listed");
    let listed: Vec<&str> = blocks.related.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(listed, vec![live.as_str()]);
}

// ---------------------------------------------------------------------------
// The `@` picker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_mention_picker_offers_no_archived_node() {
    let (svc, _tmp) = test_service().await;
    let live = create(&svc, "text", "Tidewater notes", json!({})).await;
    let gone = create(&svc, "text", "Tidewater notes, old", json!({})).await;
    archive(&svc, &gone).await;

    let offered = svc.mention_autocomplete("Tidewater", None).await.unwrap();
    assert!(has(&offered, &live));
    assert!(!has(&offered, &gone));

    unarchive(&svc, &gone).await;
    let offered = svc.mention_autocomplete("Tidewater", None).await.unwrap();
    assert!(has(&offered, &gone), "an unarchived node is offered again");
}

/// The picker's type exclusions are the registry's `mentionable` rule, and
/// the rule follows `extends`: a user type extending an unmentionable core
/// type is left out, with no list of names to keep in step.
#[tokio::test]
async fn the_mention_picker_leaves_out_a_subtype_of_an_unmentionable_type() {
    let (svc, _tmp) = test_service().await;
    handle_create_schema(
        &svc,
        json!({ "name": "Team", "extends": "collection", "fields": [] }),
    )
    .await
    .unwrap();

    let page = create(&svc, "text", "Saltmarsh page", json!({})).await;
    let collection = create(&svc, "collection", "Saltmarsh shelf", json!({})).await;
    let team = create(&svc, "team", "Saltmarsh crew", json!({})).await;

    let offered = svc.mention_autocomplete("Saltmarsh", None).await.unwrap();
    assert!(has(&offered, &page));
    assert!(
        !has(&offered, &collection),
        "a collection is not mentionable"
    );
    assert!(
        !has(&offered, &team),
        "a type extending collection is not mentionable either"
    );
}

// ---------------------------------------------------------------------------
// The vector index
// ---------------------------------------------------------------------------

mod vector_index {
    use super::*;
    use nodespace_core::models::NewEmbedding;

    fn unit_vector() -> Vec<f32> {
        let mut vector = vec![0.0f32; 768];
        vector[0] = 1.0;
        vector
    }

    async fn embed(svc: &Arc<NodeService>, id: &str) {
        svc.store()
            .upsert_embeddings(
                id,
                vec![NewEmbedding::single_chunk(
                    id,
                    unit_vector(),
                    "hash",
                    100,
                    10,
                )],
            )
            .await
            .unwrap();
    }

    async fn knn_ids(svc: &Arc<NodeService>) -> Vec<String> {
        svc.store()
            .search_embeddings(&unit_vector(), 10, Some(0.5))
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.node_id)
            .collect()
    }

    #[tokio::test]
    async fn archiving_a_node_deletes_its_embeddings() {
        let (svc, _tmp) = test_service().await;
        let id = create(&svc, "text", "Estuary report", json!({})).await;
        embed(&svc, &id).await;
        assert!(knn_ids(&svc).await.contains(&id));

        archive(&svc, &id).await;

        assert!(
            svc.store().get_embeddings(&id).await.unwrap().is_empty(),
            "an archived node has no vectors"
        );
        assert!(!knn_ids(&svc).await.contains(&id));
    }

    /// The embed that was already running when the node was archived: its
    /// vectors were computed from a read taken before the archive. The write
    /// refuses them, so call order can't put an archived node back.
    #[tokio::test]
    async fn vectors_computed_before_an_archive_are_not_written_after_it() {
        let (svc, _tmp) = test_service().await;
        let id = create(&svc, "text", "Shoreline survey", json!({})).await;
        embed(&svc, &id).await;
        archive(&svc, &id).await;

        embed(&svc, &id).await;

        assert!(svc.store().get_embeddings(&id).await.unwrap().is_empty());
        assert!(!knn_ids(&svc).await.contains(&id));
    }

    /// Every write that archives a node takes its vectors with it, not only
    /// the single-node update.
    #[tokio::test]
    async fn a_bulk_update_that_archives_deletes_embeddings() {
        let (svc, _tmp) = test_service().await;
        let kept = create(&svc, "text", "Kept report", json!({})).await;
        let gone = create(&svc, "text", "Retired report", json!({})).await;
        embed(&svc, &kept).await;
        embed(&svc, &gone).await;

        svc.bulk_update(vec![
            (
                kept.clone(),
                NodeUpdate::new().with_content("Kept report, edited".to_string()),
            ),
            (
                gone.clone(),
                NodeUpdate::new().with_lifecycle_status("archived".to_string()),
            ),
        ])
        .await
        .unwrap();

        assert!(svc.store().get_embeddings(&gone).await.unwrap().is_empty());
        assert!(!svc.store().get_embeddings(&kept).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_archived_node_is_not_queued_for_embedding() {
        let (svc, _tmp) = test_service().await;
        let id = create(&svc, "text", "Marsh ledger", json!({})).await;
        archive(&svc, &id).await;
        assert!(!svc.store().has_embeddings(&id).await.unwrap());

        // The marker write refuses it, whoever asks.
        svc.store()
            .create_stale_embedding_marker(&id)
            .await
            .unwrap();
        assert!(!svc.store().has_embeddings(&id).await.unwrap());

        // Editing it queues nothing either.
        let version = svc.get_node(&id).await.unwrap().unwrap().version;
        svc.update_node(
            &id,
            version,
            NodeUpdate::new().with_content("Marsh ledger, edited".to_string()),
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!svc.store().has_embeddings(&id).await.unwrap());
    }

    #[cfg(feature = "nlp")]
    #[tokio::test]
    async fn unarchiving_a_node_queues_it_for_embedding() {
        let (svc, _tmp) = test_service().await;
        let id = create(&svc, "text", "Delta survey", json!({})).await;
        embed(&svc, &id).await;
        archive(&svc, &id).await;
        assert!(!svc.store().has_embeddings(&id).await.unwrap());

        unarchive(&svc, &id).await;

        let queued = wait_until(|| {
            let svc = svc.clone();
            let id = id.clone();
            async move {
                svc.store()
                    .get_stale_embedding_root_ids(None, 0, 3)
                    .await
                    .unwrap()
                    .contains(&id)
            }
        })
        .await;
        assert!(queued, "an unarchived node is queued to be embedded again");
    }

    /// For a child, the vector index is its root's aggregate: an archived
    /// child, and what hangs under it, is left out of it.
    #[cfg(feature = "nlp")]
    #[tokio::test]
    async fn an_archived_child_leaves_its_roots_aggregate() {
        use nodespace_core::behaviors::{NodeBehavior, TextNodeBehavior};

        let (svc, _tmp) = test_service().await;
        let root = create(&svc, "text", "Field journal", json!({})).await;
        let kept = create_under(&svc, &root, "A kept entry").await;
        let gone = create_under(&svc, &root, "A retired entry").await;
        create_under(&svc, &gone, "Detail under the retired entry").await;
        embed(&svc, &root).await;

        archive(&svc, &gone).await;

        let root_node = svc.get_node(&root).await.unwrap().unwrap();
        let aggregate = TextNodeBehavior
            .get_aggregated_content(&root_node, svc.as_ref())
            .await
            .expect("the kept child still contributes");
        assert!(aggregate.contains("A kept entry"));
        assert!(!aggregate.contains("A retired entry"));
        assert!(!aggregate.contains("Detail under the retired entry"));
        let _ = kept;

        // The root is queued so its vector is rebuilt without the child.
        let requeued = wait_until(|| {
            let svc = svc.clone();
            let root = root.clone();
            async move {
                svc.store()
                    .get_stale_embedding_root_ids(None, 0, 3)
                    .await
                    .unwrap()
                    .contains(&root)
            }
        })
        .await;
        assert!(requeued, "the root's aggregate is rebuilt");
    }
}

// ---------------------------------------------------------------------------
// Search and the agent's context (the embedding service is behind `nlp`)
// ---------------------------------------------------------------------------

#[cfg(feature = "nlp")]
mod search_and_context {
    use super::*;
    use nodespace_core::ops::context_ops::build_workspace_context;
    use nodespace_core::ops::search_ops::{search_semantic, SearchSemanticInput};
    use nodespace_core::services::{NodeAccessor, NodeEmbeddingService};
    use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};

    /// An embedding service over an engine with no model loaded: the vector
    /// leg of a search is unavailable, which leaves the enumerate and keyword
    /// legs to answer.
    fn embedding_service(svc: &Arc<NodeService>) -> Arc<NodeEmbeddingService> {
        let engine = Arc::new(EmbeddingService::new(EmbeddingConfig::default()).unwrap());
        let accessor: Arc<dyn NodeAccessor> = Arc::new(svc.as_ref().clone());
        Arc::new(NodeEmbeddingService::new(
            engine,
            svc.store().clone(),
            accessor,
            svc.behaviors().clone(),
        ))
    }

    fn search(query: &str, include_archived: Option<bool>, keyword: bool) -> SearchSemanticInput {
        SearchSemanticInput {
            query: query.to_string(),
            threshold: None,
            limit: Some(100),
            collection_id: None,
            collection: None,
            exclude_collections: None,
            include_markdown: Some(0),
            include_archived,
            scope: None,
            node_types: Some(vec!["text".to_string()]),
            property_filters: None,
            include_edges: None,
            graph_boost: None,
            include_title_matches: Some(keyword),
        }
    }

    #[tokio::test]
    async fn search_leaves_an_archived_node_out() {
        let (svc, _tmp) = test_service().await;
        let embedding = embedding_service(&svc);
        let live = create(&svc, "text", "Breakwater inspection", json!({})).await;
        let gone = create(&svc, "text", "Breakwater inspection, old", json!({})).await;
        archive(&svc, &gone).await;

        // (query, keyword leg): the enumerate form and the keyword form.
        for (query, keyword) in [("*", false), ("Breakwater", true)] {
            let default = search_semantic(&svc, &embedding, search(query, None, keyword))
                .await
                .unwrap();
            assert!(has(&default.matched_nodes, &live), "query={query}");
            assert!(!has(&default.matched_nodes, &gone), "query={query}");
            assert!(!default.include_archived);

            let opted_in = search_semantic(&svc, &embedding, search(query, Some(true), keyword))
                .await
                .unwrap();
            assert!(has(&opted_in.matched_nodes, &gone), "query={query}");
        }
    }

    #[tokio::test]
    async fn the_workspace_context_names_no_archived_collection_or_play() {
        let (svc, _tmp) = test_service().await;
        let collection = create(&svc, "collection", "Tidal records", json!({})).await;
        let play = create(
            &svc,
            "play",
            "retitle-new-tasks",
            json!({ "rules": [{
                "name": "retitle",
                "description": "Test rule",
                "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "task" } },
                "conditions": [],
                "actions": [{
                    "description": "Test action",
                    "action_type": "update_node",
                    "params": { "node_id": "{trigger.node.id}", "content": "Retitled" }
                }]
            }] }),
        )
        .await;

        let context = build_workspace_context(&svc, None, None, None)
            .await
            .unwrap();
        assert!(context
            .collections
            .iter()
            .any(|c| c.name == "Tidal records"));
        assert!(context
            .active_playbooks
            .iter()
            .any(|p| p.name == "retitle-new-tasks"));

        archive(&svc, &collection).await;
        archive(&svc, &play).await;

        let context = build_workspace_context(&svc, None, None, None)
            .await
            .unwrap();
        assert!(!context
            .collections
            .iter()
            .any(|c| c.name == "Tidal records"));
        assert!(!context
            .active_playbooks
            .iter()
            .any(|p| p.name == "retitle-new-tasks"));
    }

    /// A play the user switched off, or the engine suspended, is not active
    /// automation, so the workspace context names neither.
    #[tokio::test]
    async fn the_workspace_context_names_only_the_plays_that_run() {
        let (svc, _tmp) = test_service().await;
        // The plays this test created; the seeded core play is listed too.
        let names = |context: &nodespace_core::ops::context_ops::WorkspaceContext| {
            context
                .active_playbooks
                .iter()
                .filter(|p| ["running", "switched-off", "suspended"].contains(&p.name.as_str()))
                .map(|p| (p.name.clone(), p.description.clone()))
                .collect::<Vec<_>>()
        };
        let running = create(
            &svc,
            "play",
            "running",
            json!({ "rules": [], "description": "Runs" }),
        )
        .await;
        create(
            &svc,
            "play",
            "switched-off",
            json!({ "rules": [], "enabled": false }),
        )
        .await;
        let suspended = create(&svc, "play", "suspended", json!({ "rules": [] })).await;
        svc.record_play_suspension(
            &suspended,
            nodespace_core::models::PlaySuspensionReason::ActionFailed,
            "boom",
        )
        .await
        .unwrap();

        let context = build_workspace_context(&svc, None, None, None)
            .await
            .unwrap();
        assert_eq!(
            names(&context),
            vec![("running".to_string(), "Runs".to_string())]
        );

        // Switching the running one off takes it out too.
        let version = svc.get_node(&running).await.unwrap().unwrap().version;
        svc.update_node(
            &running,
            version,
            NodeUpdate::default().with_properties(json!({ "enabled": false })),
        )
        .await
        .unwrap();
        let context = build_workspace_context(&svc, None, None, None)
            .await
            .unwrap();
        assert!(names(&context).is_empty());
    }
}

// ---------------------------------------------------------------------------
// The play engine
// ---------------------------------------------------------------------------

async fn wait_until<F, Fut>(mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..80 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

async fn spawn_engine(
    svc: &Arc<NodeService>,
) -> (watch::Sender<bool>, tokio::task::JoinHandle<Result<()>>) {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let engine = Arc::new(PlaybookEngine::new(Arc::clone(svc)));
    svc.set_playbook_lifecycle(engine.lifecycle().clone());
    let task = tokio::spawn(async move { engine.start(shutdown_rx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (shutdown_tx, task)
}

async fn shutdown_engine(
    shutdown_tx: watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<()>>,
) {
    let _ = shutdown_tx.send(true);
    let _ = timeout(Duration::from_secs(2), task).await;
}

/// A user type with two text fields: `status`, which the plays below write,
/// and `note`, which the tests edit to raise an event.
async fn seed_job_schema(svc: &Arc<NodeService>) {
    let schema = Node::new_with_id(
        "pp-job".to_string(),
        "schema".to_string(),
        "pp-job".to_string(),
        json!({
            "isCore": false,
            "schemaVersion": 1,
            "description": "pp-job schema",
            "fields": [
                { "name": "status", "type": "text" },
                { "name": "note", "type": "text" }
            ],
            "relationships": []
        }),
    );
    svc.create_node(schema).await.unwrap();
}

async fn create_job(svc: &Arc<NodeService>, content: &str) -> String {
    create(svc, "pp-job", content, json!({ "status": "open" })).await
}

async fn job_status(svc: &Arc<NodeService>, id: &str) -> String {
    svc.get_node(id)
        .await
        .unwrap()
        .expect("the job exists")
        .properties
        .get("pp-job")
        .and_then(|p| p.get("status"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

async fn status_becomes(svc: &Arc<NodeService>, id: &str, status: &str) -> bool {
    wait_until(|| {
        let svc = svc.clone();
        let id = id.to_string();
        let status = status.to_string();
        async move { job_status(&svc, &id).await == status }
    })
    .await
}

/// A play that marks every new `pp-job` done.
fn close_on_create() -> serde_json::Value {
    json!({ "rules": [{
        "name": "close-on-create",
        "description": "Test rule",
        "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "pp-job" } },
        "conditions": [],
        "actions": [{
            "description": "Test action",
            "action_type": "update_node",
            "params": { "node_id": "{trigger.node.id}", "properties": { "status": "done" } }
        }]
    }] })
}

/// A second type and a play on it, used as a barrier: the engine handles
/// events in order, so once a probe created now has been marked, every event
/// raised before it has been handled and every rule those events matched has
/// run. A test that asserts something did NOT happen waits on the barrier
/// instead of on a clock.
async fn seed_probe(svc: &Arc<NodeService>) {
    let schema = Node::new_with_id(
        "pp-probe".to_string(),
        "schema".to_string(),
        "pp-probe".to_string(),
        json!({
            "isCore": false,
            "schemaVersion": 1,
            "description": "pp-probe schema",
            "fields": [{ "name": "seen", "type": "text" }],
            "relationships": []
        }),
    );
    svc.create_node(schema).await.unwrap();
    create(
        svc,
        "play",
        "mark-probes",
        json!({ "rules": [{
            "name": "mark-probes",
            "description": "Test rule",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "pp-probe" } },
            "conditions": [],
            "actions": [{
                "description": "Test action",
                "action_type": "update_node",
                "params": { "node_id": "{trigger.node.id}", "properties": { "seen": "yes" } }
            }]
        }] }),
    )
    .await;
}

/// Wait until the engine has handled everything raised so far. See
/// [`seed_probe`].
async fn settled(svc: &Arc<NodeService>) {
    let probe = create(svc, "pp-probe", "probe", json!({})).await;
    let seen = wait_until(|| {
        let svc = svc.clone();
        let probe = probe.clone();
        async move {
            svc.get_node(&probe)
                .await
                .unwrap()
                .and_then(|n| {
                    n.properties
                        .get("pp-probe")
                        .and_then(|p| p.get("seen"))
                        .and_then(|v| v.as_str())
                        .map(|seen| seen == "yes")
                })
                .unwrap_or(false)
        }
    })
    .await;
    assert!(seen, "the engine handled the probe");
}

#[tokio::test]
async fn an_archived_play_does_not_run() {
    let (svc, _tmp) = test_service().await;
    seed_job_schema(&svc).await;
    let (shutdown_tx, task) = spawn_engine(&svc).await;
    seed_probe(&svc).await;

    let play = create(&svc, "play", "close-on-create", close_on_create()).await;
    let first = create_job(&svc, "Runs while the play takes part").await;
    assert!(status_becomes(&svc, &first, "done").await);

    archive(&svc, &play).await;
    let second = create_job(&svc, "Created while the play is archived").await;
    settled(&svc).await;
    assert_eq!(job_status(&svc, &second).await, "open");

    unarchive(&svc, &play).await;
    let third = create_job(&svc, "Created after the play is unarchived").await;
    assert!(
        status_becomes(&svc, &third, "done").await,
        "an unarchived play runs again"
    );

    shutdown_engine(shutdown_tx, task).await;
}

/// The startup load asks the same check as a live update does.
#[tokio::test]
async fn an_archived_play_is_not_loaded_at_startup() {
    let (svc, _tmp) = test_service().await;
    seed_job_schema(&svc).await;
    seed_probe(&svc).await;
    let play = create(&svc, "play", "close-on-create", close_on_create()).await;
    archive(&svc, &play).await;

    let (shutdown_tx, task) = spawn_engine(&svc).await;
    let job = create_job(&svc, "Created with only an archived play installed").await;
    settled(&svc).await;
    assert_eq!(job_status(&svc, &job).await, "open");

    shutdown_engine(shutdown_tx, task).await;
}

/// The event path: a rule that fires on a property change fires on a
/// participating node and not on an archived one.
#[tokio::test]
async fn no_rule_fires_on_an_archived_node() {
    let (svc, _tmp) = test_service().await;
    seed_job_schema(&svc).await;
    let (shutdown_tx, task) = spawn_engine(&svc).await;
    seed_probe(&svc).await;

    create(
        &svc,
        "play",
        "close-on-note",
        json!({ "rules": [{
            "name": "close-on-note",
            "description": "Test rule",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "select": { "target_type": "pp-job" },
                "property_key": "pp-job.note"
            },
            "conditions": [],
            "actions": [{
                "description": "Test action",
                "action_type": "update_node",
                "params": { "node_id": "{trigger.node.id}", "properties": { "status": "done" } }
            }]
        }] }),
    )
    .await;

    let live = create_job(&svc, "A live job").await;
    let gone = create_job(&svc, "A retired job").await;
    archive(&svc, &gone).await;

    for id in [&gone, &live] {
        let version = svc.get_node(id).await.unwrap().unwrap().version;
        svc.update_node(
            id,
            version,
            NodeUpdate::new().with_properties(json!({ "note": "checked" })),
        )
        .await
        .unwrap();
    }

    assert!(
        status_becomes(&svc, &live, "done").await,
        "the rule fires on the participating node"
    );
    settled(&svc).await;
    assert_eq!(
        job_status(&svc, &gone).await,
        "open",
        "no rule fires on an archived node"
    );

    shutdown_engine(shutdown_tx, task).await;
}

/// The action path: a rule that runs on a participating trigger touches no
/// archived node, whichever action names it. The rule's last action proves
/// it ran to the end.
#[tokio::test]
async fn no_action_touches_an_archived_node() {
    let (svc, _tmp) = test_service().await;
    seed_job_schema(&svc).await;
    let target = create_job(&svc, "The archived target").await;
    let shelf = create(&svc, "collection", "The archived shelf", json!({})).await;
    let filed = create_job(&svc, "Filed on the shelf before it was archived").await;
    svc.create_relationship(&filed, "member_of", &shelf, json!({}))
        .await
        .unwrap();
    archive(&svc, &target).await;
    archive(&svc, &shelf).await;

    let (shutdown_tx, task) = spawn_engine(&svc).await;
    create(
        &svc,
        "play",
        "touch-the-archived",
        json!({ "rules": [{
            "name": "touch-the-archived",
            "description": "Test rule",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "pp-job" } },
            "conditions": [],
            "actions": [
                {
                    "description": "Test action",
                    "action_type": "update_node",
                    "params": { "node_id": target, "properties": { "status": "touched" } }
                },
                {
                    "description": "Test action",
                    "action_type": "add_relationship",
                    "params": {
                        "source_id": "{trigger.node.id}",
                        "relationship_type": "member_of",
                        "target_id": shelf
                    }
                },
                {
                    "description": "Test action",
                    "action_type": "remove_relationship",
                    "params": {
                        "source_id": filed,
                        "relationship_type": "member_of",
                        "target_id": shelf
                    }
                },
                {
                    "description": "Test action",
                    "action_type": "update_node",
                    "params": { "node_id": "{trigger.node.id}", "properties": { "status": "done" } }
                }
            ]
        }] }),
    )
    .await;

    let trigger = create_job(&svc, "The trigger").await;
    assert!(
        status_becomes(&svc, &trigger, "done").await,
        "the rule ran to its last action"
    );
    assert_eq!(
        job_status(&svc, &target).await,
        "open",
        "the archived target is not updated"
    );
    let store = svc.store();
    assert!(
        store
            .get_node_memberships(&trigger)
            .await
            .unwrap()
            .is_empty(),
        "no edge is added to an archived node"
    );
    assert_eq!(
        store.get_node_memberships(&filed).await.unwrap(),
        vec![shelf.clone()],
        "no edge is removed from an archived node"
    );

    shutdown_engine(shutdown_tx, task).await;
}

/// A related node that is archived is absent from what a rule reads: it is
/// in no collection a condition quantifies over, so an archived, unfinished
/// part doesn't hold its parent open.
#[tokio::test]
async fn a_condition_sees_no_archived_related_node() {
    let (svc, _tmp) = test_service().await;
    seed_job_schema(&svc).await;
    let schema = Node::new_with_id(
        "pp-batch".to_string(),
        "schema".to_string(),
        "pp-batch".to_string(),
        json!({
            "isCore": false,
            "schemaVersion": 1,
            "description": "pp-batch schema",
            "fields": [
                { "name": "status", "type": "text" },
                { "name": "note", "type": "text" }
            ],
            "relationships": []
        }),
    );
    svc.create_node(schema).await.unwrap();
    let declarations: Vec<nodespace_core::models::schema::SchemaRelationship> =
        serde_json::from_value(json!([{
            "name": "jobs",
            "targetType": "pp-job",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "batch",
            "reverseCardinality": "one"
        }]))
        .unwrap();
    svc.set_schema_relationships("pp-batch", &declarations)
        .await
        .unwrap();

    let (shutdown_tx, task) = spawn_engine(&svc).await;
    create(
        &svc,
        "play",
        "close-finished-batches",
        json!({ "rules": [{
            "name": "close-finished-batches",
            "description": "Test rule",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "select": { "target_type": "pp-batch" },
                "property_key": "pp-batch.note"
            },
            "conditions": [{ "expr": "node.jobs.all(j, j.status == 'done')", "description": "Test condition" }],
            "actions": [{
                "description": "Test action",
                "action_type": "update_node",
                "params": { "node_id": "{trigger.node.id}", "properties": { "status": "done" } }
            }]
        }] }),
    )
    .await;

    let batch = create(&svc, "pp-batch", "A batch", json!({ "status": "open" })).await;
    let finished = create(&svc, "pp-job", "Finished", json!({ "status": "done" })).await;
    let abandoned = create_job(&svc, "Abandoned, still open").await;
    for job in [&finished, &abandoned] {
        svc.create_relationship(&batch, "jobs", job, json!({}))
            .await
            .unwrap();
    }
    archive(&svc, &abandoned).await;

    let version = svc.get_node(&batch).await.unwrap().unwrap().version;
    svc.update_node(
        &batch,
        version,
        NodeUpdate::new().with_properties(json!({ "note": "checked" })),
    )
    .await
    .unwrap();

    let closed = wait_until(|| {
        let svc = svc.clone();
        let batch = batch.clone();
        async move {
            svc.get_node(&batch)
                .await
                .unwrap()
                .and_then(|n| {
                    n.properties
                        .get("pp-batch")
                        .and_then(|p| p.get("status"))
                        .and_then(|v| v.as_str())
                        .map(|status| status == "done")
                })
                .unwrap_or(false)
        }
    })
    .await;
    assert!(closed, "the archived job doesn't count against the batch");

    shutdown_engine(shutdown_tx, task).await;
}

// ---------------------------------------------------------------------------
// Invariant rules: synchronous, in the write's own transaction
// ---------------------------------------------------------------------------

/// Activate invariant rules directly in the lifecycle manager, as the engine
/// does at load. No running engine: an invariant rule runs inside the write
/// that triggers it, so these tests have nothing to wait for.
fn activate_invariant_rules(svc: &Arc<NodeService>, rules: serde_json::Value) -> PlaybookEngine {
    let engine = PlaybookEngine::new(Arc::clone(svc));
    svc.set_playbook_lifecycle(engine.lifecycle().clone());
    let play = Node::new(
        "play".to_string(),
        "invariants".to_string(),
        json!({ "play": { "rules": rules } }),
    );
    engine
        .lifecycle()
        .write()
        .unwrap()
        .activate_play(&play)
        .expect("the play parses and activates");
    engine
}

async fn create_archived_job(svc: &Arc<NodeService>, properties: serde_json::Value) -> String {
    svc.create_node_with_parent(CreateNodeParams {
        id: None,
        node_type: "pp-job".to_string(),
        content: "Created archived".to_string(),
        parent_id: None,
        position: InsertPositionOwned::End,
        properties,
        lifecycle_status: Some("archived".to_string()),
    })
    .await
    .expect("an archived node is created like any node")
}

/// No rule fires on an archived node, an invariant included: a write to one
/// is not a play's to veto.
#[tokio::test]
async fn no_invariant_rule_fires_on_an_archived_node() {
    let (svc, _tmp) = test_service().await;
    seed_job_schema(&svc).await;
    let _engine = activate_invariant_rules(
        &svc,
        json!([{
            "name": "no-forbidden-jobs",
            "class": "invariant",
            "description": "Test rule",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "pp-job" } },
            "conditions": [{ "expr": "node.status == 'forbidden'", "description": "Test condition" }],
            "actions": [{ "description": "Test action", "action_type": "reject", "params": { "message": "forbidden" } }]
        }]),
    );

    let refused = svc
        .create_node(Node::new(
            "pp-job".to_string(),
            "A live forbidden job".to_string(),
            json!({ "status": "forbidden" }),
        ))
        .await;
    assert!(refused.is_err(), "the rule vetoes a participating node");

    let archived = create_archived_job(&svc, json!({ "status": "forbidden" })).await;
    assert_eq!(job_status(&svc, &archived).await, "forbidden");
}

/// The in-transaction action path: an invariant rule that runs on a
/// participating trigger touches no archived node, whichever action names it.
#[tokio::test]
async fn no_invariant_action_touches_an_archived_node() {
    let (svc, _tmp) = test_service().await;
    seed_job_schema(&svc).await;
    let target = create_job(&svc, "The archived target").await;
    let shelf = create(&svc, "collection", "The archived shelf", json!({})).await;
    let filed = create_job(&svc, "Filed on the shelf before it was archived").await;
    svc.create_relationship(&filed, "member_of", &shelf, json!({}))
        .await
        .unwrap();
    archive(&svc, &target).await;
    archive(&svc, &shelf).await;

    let _engine = activate_invariant_rules(
        &svc,
        json!([{
            "name": "touch-the-archived",
            "class": "invariant",
            "description": "Test rule",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "pp-job" } },
            "conditions": [],
            "actions": [
                {
                    "description": "Test action",
                    "action_type": "update_node",
                    "params": { "node_id": target, "properties": { "status": "touched" } }
                },
                {
                    "description": "Test action",
                    "action_type": "add_relationship",
                    "params": {
                        "source_id": "{trigger.node.id}",
                        "relationship_type": "member_of",
                        "target_id": shelf,
                        "edge_data": { "order": 1.0 }
                    }
                },
                {
                    "description": "Test action",
                    "action_type": "remove_relationship",
                    "params": {
                        "source_id": filed,
                        "relationship_type": "member_of",
                        "target_id": shelf
                    }
                },
                {
                    "description": "Test action",
                    "action_type": "update_node",
                    "params": { "node_id": "{trigger.node.id}", "properties": { "status": "done" } }
                }
            ]
        }]),
    );

    let trigger = create_job(&svc, "The trigger").await;
    assert_eq!(
        job_status(&svc, &trigger).await,
        "done",
        "the rule ran to its last action, inside the create"
    );
    assert_eq!(job_status(&svc, &target).await, "open");
    let store = svc.store();
    assert!(store
        .get_node_memberships(&trigger)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.get_node_memberships(&filed).await.unwrap(),
        vec![shelf.clone()]
    );
}

// ---------------------------------------------------------------------------
// Plays neither read nor write the lifecycle (ADR-087 §5)
// ---------------------------------------------------------------------------

async fn save_play(svc: &Arc<NodeService>, rule: serde_json::Value) -> Result<String, String> {
    svc.create_node(Node::new(
        "play".to_string(),
        "a play".to_string(),
        json!({ "rules": [rule] }),
    ))
    .await
    .map_err(|e| e.to_string())
}

#[tokio::test]
async fn a_condition_naming_the_lifecycle_fails_validation() {
    let (svc, _tmp) = test_service().await;
    seed_job_schema(&svc).await;

    for condition in [
        "node.lifecycle_status == 'active'",
        "node.status == 'open' && node.lifecycle_status != 'archived'",
    ] {
        let error = save_play(
            &svc,
            json!({
                "name": "reads-lifecycle",
                "description": "Test rule",
                "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "pp-job" } },
                "conditions": [{ "expr": condition, "description": "Test condition" }],
                "actions": [{
                    "description": "Test action",
                    "action_type": "update_node",
                    "params": { "node_id": "{trigger.node.id}", "properties": { "status": "done" } }
                }]
            }),
        )
        .await
        .expect_err("a play can't read a node's lifecycle");
        assert!(error.contains("lifecycle"), "{condition}: {error}");
        assert!(
            error.contains("rule[0].condition[0]"),
            "{condition}: {error}"
        );
    }
}

#[tokio::test]
async fn an_update_node_action_carrying_the_lifecycle_is_rejected() {
    let (svc, _tmp) = test_service().await;
    seed_job_schema(&svc).await;

    let error = save_play(
        &svc,
        json!({
            "name": "archives",
            "description": "Test rule",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "pp-job" } },
            "conditions": [],
            "actions": [{
                "description": "Test action",
                "action_type": "update_node",
                "params": { "node_id": "{trigger.node.id}", "lifecycle_status": "archived" }
            }]
        }),
    )
    .await
    .expect_err("no play action takes the lifecycle as a parameter");
    // The typed `update_node` params have no such field, so the rule does not
    // decode: the error names the rule and the param.
    assert!(
        error.contains("unknown field `lifecycle_status`"),
        "{error}"
    );
    assert!(error.contains("rule[0] ('archives')"), "{error}");
}

/// A binding reads a node's wire JSON, where the field is spelled
/// `lifecycleStatus`. Both spellings are refused, in a param's `{binding}`
/// and in a `for_each` path.
#[tokio::test]
async fn a_binding_naming_the_lifecycle_fails_validation() {
    let (svc, _tmp) = test_service().await;
    seed_job_schema(&svc).await;

    for binding in [
        "{trigger.node.lifecycleStatus}",
        "{trigger.node.lifecycle_status}",
    ] {
        let error = save_play(
            &svc,
            json!({
                "name": "copies-lifecycle",
                "description": "Test rule",
                "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "pp-job" } },
                "conditions": [],
                "actions": [{
                    "description": "Test action",
                    "action_type": "update_node",
                    "params": { "node_id": "{trigger.node.id}", "properties": { "note": binding } }
                }]
            }),
        )
        .await
        .expect_err("a binding can't read a node's lifecycle");
        assert!(error.contains("lifecycle"), "{binding}: {error}");
        assert!(
            error.contains("rule[0].action[0].params"),
            "{binding}: {error}"
        );
    }

    let error = save_play(
        &svc,
        json!({
            "name": "iterates-lifecycle",
            "description": "Test rule",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "pp-job" } },
            "conditions": [],
            "actions": [{
                "description": "Test action",
                "action_type": "update_node",
                "for_each": "trigger.node.lifecycleStatus",
                "params": { "node_id": "{trigger.node.id}", "properties": { "note": "x" } }
            }]
        }),
    )
    .await
    .expect_err("a for_each path can't read a node's lifecycle");
    assert!(error.contains("rule[0].action[0].for_each"), "{error}");
}

/// Refusing the name at save time is the check; this is the guarantee under
/// it. The JSON a binding navigates carries no lifecycle, so a rule already
/// in the engine can't read it either.
#[tokio::test]
async fn a_binding_cannot_reach_the_lifecycle_at_run_time() {
    let (svc, _tmp) = test_service().await;
    seed_job_schema(&svc).await;
    // Activated directly, past save-time validation.
    let _engine = activate_invariant_rules(
        &svc,
        json!([{
            "name": "copies-lifecycle",
            "class": "invariant",
            "description": "Test rule",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "pp-job" } },
            "conditions": [],
            "actions": [{
                "description": "Test action",
                "action_type": "update_node",
                "params": {
                    "node_id": "{trigger.node.id}",
                    "properties": { "note": "{trigger.node.lifecycleStatus}" }
                }
            }]
        }]),
    );

    let error = svc
        .create_node(Node::new(
            "pp-job".to_string(),
            "A job".to_string(),
            json!({ "status": "open" }),
        ))
        .await
        .expect_err("the binding has nothing to resolve, so the invariant rule fails the write")
        .to_string();
    assert!(error.contains("lifecycleStatus"), "{error}");
}
