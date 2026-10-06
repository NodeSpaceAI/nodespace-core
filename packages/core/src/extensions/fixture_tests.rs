//! The data extension points used the way another build uses them (ADR-082
//! §2, §9):
//!
//! - a fixture subtype of `collection` with its own behaviour: validation by
//!   its behaviour and its base's, fields in its own bucket, queries for
//!   `collection` returning it, and retyping a collection to it;
//! - a fixture edge field on `member_of`, in the extension's bucket of the
//!   edge's properties: stored there, validated on every edge write, and a
//!   bucket nobody registered left alone.
//!
//! This file tests the data half of the extension points (ADR-082 §9).

use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;

use super::{DataExtensions, EdgeFieldDeclaration};
use crate::behaviors::{CollectionNodeBehavior, NodeBehavior};
use crate::db::SqliteStore;
use crate::models::schema::{EdgeField, EnumValue, SchemaFieldType};
use crate::models::{Node, NodeUpdate, ValidationError as NodeValidationError};
use crate::services::{CollectionService, CreateNodeParams, InsertPositionOwned, NodeService};

/// The fixture subtype's schema id.
const FIXTURE_TYPE: &str = "fixture-collection";

/// The behaviour of `fixture-collection extends collection`. It reads its own
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
            "fixture-collection": { "max_members": 3 },
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
            "fixture-collection": { "max_members": 5 },
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

// ---------------------------------------------------------------------------
// Edge fields (ADR-082 §2.2)
// ---------------------------------------------------------------------------

/// The fixture's extension id, which names its bucket.
const FIXTURE_ID: &str = "fixture";

fn edge_field(
    name: &str,
    field_type: SchemaFieldType,
    values: &[&str],
    required: bool,
) -> EdgeField {
    EdgeField {
        name: name.to_string(),
        field_type,
        core_values: (!values.is_empty())
            .then(|| values.iter().map(|v| EnumValue::new(*v, *v)).collect()),
        indexed: None,
        required: required.then_some(true),
        default: None,
        target_type: None,
        description: None,
    }
}

/// The fixture's fields on `member_of`: a required `permission` enum and an
/// optional `since` date, with a rule between them their types cannot
/// express.
fn member_of_fields() -> EdgeFieldDeclaration {
    EdgeFieldDeclaration::new(
        "member_of",
        FIXTURE_ID,
        vec![
            edge_field(
                "permission",
                SchemaFieldType::Enum,
                &["admin", "modify", "read_only"],
                true,
            ),
            edge_field("since", SchemaFieldType::Date, &[], false),
        ],
    )
    .with_validator(|bucket| {
        let admin = bucket.get("permission") == Some(&json!("admin"));
        let since = bucket.get("since").is_some_and(|v| !v.is_null());
        if admin && !since {
            Err("an admin membership records since when".to_string())
        } else {
            Ok(())
        }
    })
}

async fn edge_service(extensions: DataExtensions) -> (Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let mut store = Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
    let svc = Arc::new(
        NodeService::new_with_extensions(&mut store, &extensions)
            .await
            .unwrap(),
    );
    (svc, tmp)
}

/// A collection and a text node to put in it.
async fn member_and_collection(svc: &NodeService) -> (String, String) {
    let collection = create(svc, "collection", "Team", json!({})).await.unwrap();
    let member = create(svc, "text", "Notes", json!({})).await.unwrap();
    (member.id, collection.id)
}

async fn stored_edge(svc: &NodeService, source: &str, target: &str) -> Option<serde_json::Value> {
    svc.store()
        .get_relationship_record(source, target, "member_of")
        .await
        .unwrap()
        .map(|record| record.properties)
}

/// The fixture's fields are stored in its bucket of the edge, beside core's
/// `order`, on both of `create_relationship`'s `member_of` paths: the
/// auto-ordered one and the one with an explicit order.
#[tokio::test]
async fn an_edge_field_on_member_of_is_stored_in_the_extension_bucket() {
    let (svc, _tmp) = edge_service(DataExtensions::none().edge_fields(member_of_fields())).await;
    let (member, collection) = member_and_collection(&svc).await;
    let other = create(&svc, "text", "More notes", json!({})).await.unwrap();

    svc.create_relationship(
        &member,
        "member_of",
        &collection,
        json!({ "fixture": { "permission": "modify" } }),
    )
    .await
    .unwrap();
    let stored = stored_edge(&svc, &member, &collection).await.unwrap();
    assert_eq!(stored["fixture"], json!({ "permission": "modify" }));
    assert!(
        stored.get("order").is_some(),
        "core's order is kept: {stored}"
    );

    let bucket = json!({ "permission": "admin", "since": "2026-10-01" });
    svc.create_relationship(
        &other.id,
        "member_of",
        &collection,
        json!({ "order": 5.0, "fixture": bucket }),
    )
    .await
    .unwrap();
    assert_eq!(
        stored_edge(&svc, &other.id, &collection).await.unwrap(),
        json!({ "order": 5.0, "fixture": bucket })
    );
}

/// Every way the fixture's bucket can be wrong, refused before anything is
/// stored.
#[tokio::test]
async fn an_invalid_bucket_is_refused_on_create() {
    let (svc, _tmp) = edge_service(DataExtensions::none().edge_fields(member_of_fields())).await;
    let (member, collection) = member_and_collection(&svc).await;

    let cases = [
        (
            json!({ "fixture": { "permission": "owner" } }),
            "one of: admin, modify, read_only",
        ),
        (json!({ "fixture": "modify" }), "must be a JSON object"),
        (
            json!({ "fixture": { "permission": "modify", "level": 2 } }),
            "'level' is not a field",
        ),
        (
            json!({ "fixture": { "since": "2026-10-01" } }),
            "'fixture.permission' is required",
        ),
        (
            json!({ "fixture": { "permission": "modify", "since": 5 } }),
            "must be a date",
        ),
        (
            json!({ "fixture": { "permission": "admin" } }),
            "an admin membership records since when",
        ),
    ];
    for (edge_data, expected) in cases {
        let refused = svc
            .create_relationship(&member, "member_of", &collection, edge_data.clone())
            .await;
        assert!(
            refused
                .as_ref()
                .is_err_and(|e| e.to_string().contains(expected)),
            "{edge_data}: expected '{expected}', got {refused:?}"
        );
        assert_eq!(
            stored_edge(&svc, &member, &collection).await,
            None,
            "{edge_data}"
        );
    }
}

/// The transactional write path, which a Play's `add_relationship` action
/// and the multi-edge create use, validates the bucket too.
#[tokio::test]
async fn an_invalid_bucket_is_refused_in_a_transaction() {
    let (svc, _tmp) = edge_service(DataExtensions::none().edge_fields(member_of_fields())).await;
    let (member, collection) = member_and_collection(&svc).await;

    let in_tx = svc.clone();
    let (source, target) = (member.clone(), collection.clone());
    let refused = svc
        .with_transaction(move |tx| {
            Box::pin(async move {
                in_tx
                    .create_relationship_in_tx(
                        tx,
                        &source,
                        "member_of",
                        &target,
                        json!({ "order": 1.0, "fixture": { "permission": "owner" } }),
                    )
                    .await
            })
        })
        .await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.to_string().contains("one of: admin, modify, read_only")),
        "{:?}",
        refused.map(|_| ())
    );
    assert_eq!(stored_edge(&svc, &member, &collection).await, None);
}

/// Replacing an edge's properties validates the bucket, and so does
/// creating an edge that already exists, which stores nothing.
#[tokio::test]
async fn an_invalid_bucket_is_refused_on_update_and_on_an_existing_edge() {
    let (svc, _tmp) = edge_service(DataExtensions::none().edge_fields(member_of_fields())).await;
    let (member, collection) = member_and_collection(&svc).await;
    let valid = json!({ "order": 1.0, "fixture": { "permission": "read_only" } });
    svc.create_relationship(&member, "member_of", &collection, valid.clone())
        .await
        .unwrap();

    let refused = svc
        .update_relationship_properties(
            &member,
            "member_of",
            &collection,
            json!({ "order": 1.0, "fixture": { "permission": "owner" } }),
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
    let refused = svc
        .create_relationship(
            &member,
            "member_of",
            &collection,
            json!({ "fixture": { "permission": "owner" } }),
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(stored_edge(&svc, &member, &collection).await, Some(valid));

    let updated =
        json!({ "order": 1.0, "fixture": { "permission": "admin", "since": "2026-10-05" } });
    svc.update_relationship_properties(&member, "member_of", &collection, updated.clone())
        .await
        .unwrap();
    assert_eq!(stored_edge(&svc, &member, &collection).await, Some(updated));
}

/// Core reads only registered buckets: another bucket on the edge is stored
/// as given, the fixture's bucket on a relationship it did not declare fields
/// for is too, and without the fixture's declaration its bucket is just as
/// opaque.
#[tokio::test]
async fn a_bucket_nobody_registered_is_left_alone() {
    let (svc, _tmp) = edge_service(DataExtensions::none().edge_fields(member_of_fields())).await;
    let (member, collection) = member_and_collection(&svc).await;
    let edge = json!({ "order": 1.0, "other": { "anything": [1, "two"] } });
    svc.create_relationship(&member, "member_of", &collection, edge.clone())
        .await
        .unwrap();
    assert_eq!(stored_edge(&svc, &member, &collection).await, Some(edge));

    let mentioned = create(&svc, "text", "Mentioned", json!({})).await.unwrap();
    let mention = json!({ "fixture": { "permission": "owner" } });
    svc.create_relationship(&member, "mentions", &mentioned.id, mention.clone())
        .await
        .unwrap();
    let stored = svc
        .store()
        .get_relationship_record(&member, &mentioned.id, "mentions")
        .await
        .unwrap()
        .expect("the mention is stored");
    assert_eq!(stored.properties, mention);

    let (plain, _tmp) = edge_service(DataExtensions::none()).await;
    let (member, collection) = member_and_collection(&plain).await;
    let edge = json!({ "order": 1.0, "fixture": { "permission": "owner", "level": 2 } });
    plain
        .create_relationship(&member, "member_of", &collection, edge.clone())
        .await
        .unwrap();
    assert_eq!(stored_edge(&plain, &member, &collection).await, Some(edge));
}

/// A refused declaration fails the node service before anything is written.
#[tokio::test]
async fn a_node_service_with_a_refused_edge_field_declaration_is_refused() {
    let tmp = TempDir::new().unwrap();
    let mut store = Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
    let has_child = EdgeFieldDeclaration::new(
        "has_child",
        FIXTURE_ID,
        vec![edge_field("note", SchemaFieldType::Text, &[], false)],
    );

    let refused = NodeService::new_with_extensions(
        &mut store,
        &DataExtensions::none().edge_fields(has_child),
    )
    .await;
    assert!(
        matches!(
            refused,
            Err(crate::services::NodeServiceError::InitializationError(ref msg))
                if msg.contains("fields can be added only to member_of and has_role edges")
        ),
        "{:?}",
        refused.err()
    );
    assert!(store.get_schema("collection").await.unwrap().is_none());
}
