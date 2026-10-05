//! A fixture subtype of `collection` with its own behaviour, the way another
//! build adds one (ADR-082 §2.1, §9): validation by its behaviour and its
//! base's, fields in its own bucket, queries for `collection` returning it, and
//! retyping a collection to it.
//!
//! This file records the data half of the extension API. A change to it needs
//! an `EXTENSION_API_VERSION` bump (ADR-082 §8), which
//! `scripts/check-extension-api-version.ts` enforces.

use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;

use super::DataExtensions;
use crate::behaviors::{CollectionNodeBehavior, NodeBehavior};
use crate::db::SqliteStore;
use crate::models::{Node, NodeUpdate, ValidationError as NodeValidationError};
use crate::services::{CollectionService, CreateNodeParams, InsertPositionOwned, NodeService};

/// The fixture subtype's schema id.
const FIXTURE_TYPE: &str = "fixture_collection";

/// The behaviour of `fixture_collection extends collection`. It reads its own
/// field from its own bucket and refuses a `max_members` below 1, which the
/// schema's `number` type alone admits. For everything but validation it
/// behaves as a collection.
struct FixtureCollectionBehavior;

impl NodeBehavior for FixtureCollectionBehavior {
    fn type_name(&self) -> &'static str {
        FIXTURE_TYPE
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        let max_members = node
            .properties
            .get(FIXTURE_TYPE)
            .and_then(|bucket| bucket.get("max_members"));
        match max_members {
            None | Some(serde_json::Value::Null) => Ok(()),
            Some(value) if value.as_f64().is_some_and(|n| n >= 1.0) => Ok(()),
            Some(value) => Err(NodeValidationError::InvalidProperties(format!(
                "max_members must be at least 1, got {value}"
            ))),
        }
    }

    fn supports_markdown(&self) -> bool {
        CollectionNodeBehavior.supports_markdown()
    }

    fn get_embeddable_content(&self, node: &Node) -> Option<String> {
        CollectionNodeBehavior.get_embeddable_content(node)
    }
}

/// A node service over a fresh database with the fixture's behaviour added,
/// and, when `with_schema`, the fixture's schema created as the other build
/// creates it.
async fn service(with_schema: bool) -> (Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let mut store = Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
    let extensions = DataExtensions::none().behavior(Arc::new(FixtureCollectionBehavior));
    let svc = Arc::new(
        NodeService::new_with_extensions(&mut store, &extensions)
            .await
            .unwrap(),
    );
    if with_schema {
        crate::schema::handle_create_schema(
            &svc,
            json!({
                "name": FIXTURE_TYPE,
                "extends": "collection",
                "fields": [
                    { "name": "max_members", "type": "number", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .unwrap();
    }
    (svc, tmp)
}

fn params(node_type: &str, content: &str, properties: serde_json::Value) -> CreateNodeParams {
    CreateNodeParams {
        id: None,
        node_type: node_type.to_string(),
        content: content.to_string(),
        parent_id: None,
        position: InsertPositionOwned::End,
        properties,
        lifecycle_status: None,
    }
}

async fn create(
    svc: &NodeService,
    node_type: &str,
    content: &str,
    properties: serde_json::Value,
) -> Result<Node, crate::services::NodeServiceError> {
    let id = svc
        .create_node_with_parent(params(node_type, content, properties))
        .await?;
    Ok(svc.get_node(&id).await.unwrap().unwrap())
}

/// The subtype's behaviour runs on its own field, and its base's behaviour
/// still runs: a subtype adds rules and never relaxes them.
#[tokio::test]
async fn a_subtype_node_is_validated_by_its_behaviour_and_its_base() {
    let (svc, _tmp) = service(true).await;

    let refused = create(&svc, FIXTURE_TYPE, "Team", json!({ "max_members": 0 })).await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.to_string().contains("max_members must be at least 1")),
        "the subtype's behaviour refuses it: {refused:?}"
    );

    let refused = create(&svc, FIXTURE_TYPE, "a:b", json!({ "max_members": 3 })).await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.to_string().contains("cannot contain ':'")),
        "collection's behaviour refuses it: {refused:?}"
    );

    create(&svc, FIXTURE_TYPE, "Team", json!({ "max_members": 3 }))
        .await
        .expect("a valid fixture collection is created");
}

/// The subtype's own field is stored in its own bucket and core's field in
/// the collection bucket, whatever shape the caller wrote them in.
#[tokio::test]
async fn the_subtypes_fields_are_stored_in_its_own_bucket() {
    let (svc, _tmp) = service(true).await;

    let node = create(
        &svc,
        FIXTURE_TYPE,
        "Team",
        json!({ "max_members": 3, "description": "Our team" }),
    )
    .await
    .unwrap();

    assert_eq!(node.node_type, FIXTURE_TYPE);
    assert_eq!(
        node.properties,
        json!({
            "fixture_collection": { "max_members": 3 },
            "collection": { "description": "Our team" }
        })
    );
}

/// A query for `collection`, and core's collection listing, return the
/// subtype's nodes as collections.
#[tokio::test]
async fn queries_for_collection_return_the_subtype() {
    let (svc, _tmp) = service(true).await;
    let plain = create(&svc, "collection", "Plain", json!({}))
        .await
        .unwrap();
    let fixture = create(&svc, FIXTURE_TYPE, "Team", json!({ "max_members": 3 }))
        .await
        .unwrap();

    let mut queried: Vec<String> = svc
        .query_nodes_by_type("collection", false)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    queried.sort();
    let mut expected = vec![plain.id.clone(), fixture.id.clone()];
    expected.sort();
    assert_eq!(queried, expected);

    let store = svc.store().clone();
    let listed: Vec<String> = CollectionService::new(&store, &svc)
        .get_all_collections_with_counts()
        .await
        .unwrap()
        .into_iter()
        .map(|(n, _, _)| n.id)
        .collect();
    assert!(
        listed.contains(&fixture.id) && listed.contains(&plain.id),
        "the collection listing holds both: {listed:?}"
    );
}

/// Retyping a collection to the subtype goes through the write path's
/// rebucket and validation: the subtype's behaviour judges the result, the
/// new field lands in the subtype's bucket, and core's field stays in the
/// collection bucket.
#[tokio::test]
async fn a_collection_retyped_to_the_subtype_is_rebucketed_and_validated() {
    let (svc, _tmp) = service(true).await;
    let node = create(
        &svc,
        "collection",
        "Team",
        json!({ "description": "Our team" }),
    )
    .await
    .unwrap();

    let refused = svc
        .update_node(
            &node.id,
            node.version,
            NodeUpdate::new()
                .with_node_type(FIXTURE_TYPE.to_string())
                .with_properties(json!({ "max_members": 0 })),
        )
        .await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.to_string().contains("max_members must be at least 1")),
        "the retype is judged by the subtype's behaviour: {refused:?}"
    );
    let unchanged = svc.get_node(&node.id).await.unwrap().unwrap();
    assert_eq!(unchanged.node_type, "collection");

    svc.update_node(
        &node.id,
        node.version,
        NodeUpdate::new()
            .with_node_type(FIXTURE_TYPE.to_string())
            .with_properties(json!({ "max_members": 5 })),
    )
    .await
    .expect("the retype is accepted");

    let retyped = svc.get_node(&node.id).await.unwrap().unwrap();
    assert_eq!(retyped.node_type, FIXTURE_TYPE);
    assert_eq!(
        retyped.properties,
        json!({
            "fixture_collection": { "max_members": 5 },
            "collection": { "description": "Our team" }
        })
    );
    let collections = svc.query_nodes_by_type("collection", false).await.unwrap();
    assert!(collections.iter().any(|n| n.id == node.id));
}

/// A behaviour does not make its type known: until the subtype's schema
/// exists in the database, a node of the type is refused rather than stored
/// as a type that is not a collection, on every create path and on a retype.
#[tokio::test]
async fn a_subtype_without_its_schema_is_an_unknown_type() {
    fn is_unknown<T: std::fmt::Debug>(
        result: &Result<T, crate::services::NodeServiceError>,
    ) -> bool {
        matches!(
            result,
            Err(crate::services::NodeServiceError::UnknownNodeType { node_type })
                if node_type == FIXTURE_TYPE
        )
    }
    let (svc, _tmp) = service(false).await;

    let refused = create(&svc, FIXTURE_TYPE, "Team", json!({})).await;
    assert!(is_unknown(&refused), "create with params: {refused:?}");

    let refused = svc
        .create_node(Node::new(
            FIXTURE_TYPE.to_string(),
            "Team".to_string(),
            json!({}),
        ))
        .await;
    assert!(is_unknown(&refused), "create_node: {refused:?}");

    let node = create(&svc, "collection", "Team", json!({})).await.unwrap();
    let refused = svc
        .update_node(
            &node.id,
            node.version,
            NodeUpdate::new().with_node_type(FIXTURE_TYPE.to_string()),
        )
        .await;
    assert!(is_unknown(&refused), "retype: {refused:?}");
    let unchanged = svc.get_node(&node.id).await.unwrap().unwrap();
    assert_eq!(unchanged.node_type, "collection");
}

/// The node service validates with the behaviours it was given, and a plain
/// one with core's alone.
#[tokio::test]
async fn the_node_service_registers_the_added_behaviours() {
    let (svc, _tmp) = service(false).await;
    assert!(svc.behaviors().get(FIXTURE_TYPE).is_some());

    let tmp = TempDir::new().unwrap();
    let mut store = Arc::new(SqliteStore::new(tmp.path().join("plain.db")).await.unwrap());
    let plain = NodeService::new(&mut store).await.unwrap();
    assert!(plain.behaviors().get(FIXTURE_TYPE).is_none());
}

/// A refused extension fails the node service before anything is written.
#[tokio::test]
async fn a_node_service_with_a_behaviour_for_a_core_type_is_refused() {
    let tmp = TempDir::new().unwrap();
    let mut store = Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
    let extensions = DataExtensions::none().behavior(Arc::new(CollectionNodeBehavior));

    let refused = NodeService::new_with_extensions(&mut store, &extensions).await;
    assert!(
        matches!(
            refused,
            Err(crate::services::NodeServiceError::InitializationError(ref msg))
                if msg.contains("'collection' is a core type")
        ),
        "{:?}",
        refused.err()
    );
    assert!(
        store.get_schema("collection").await.unwrap().is_none(),
        "core's schemas were not seeded"
    );
}
