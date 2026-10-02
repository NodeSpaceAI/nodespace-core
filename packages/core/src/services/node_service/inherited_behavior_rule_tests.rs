//! A behaviour's rule on a field holds for a node whose type extends the
//! type that declares the field (ADR-086): every write path validates the
//! value it is about to store, in the bucket it will be stored in.
//!
//! The rule used throughout is `project`'s: `start_date` must be on or before
//! `end_date`. Only the behaviour enforces it, so the schema's own field
//! validation cannot stand in for it. `campaign` extends `project`, so both
//! dates are inherited and stored in the `project` bucket.

use super::crud::VersionCheckedUpdateOutcome;
use super::*;
use crate::db::SqliteStore;
use serde_json::json;
use tempfile::TempDir;

const RULE: &str = "start_date must be on or before end_date";

async fn service_with_campaign_type() -> (Arc<NodeService>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let mut store = Arc::new(
        SqliteStore::new(temp_dir.path().join("test.db"))
            .await
            .unwrap(),
    );
    let service = Arc::new(NodeService::new(&mut store).await.unwrap());
    crate::schema::handle_create_schema(
        &service,
        json!({
            "name": "Campaign",
            "extends": "project",
            "fields": [
                { "name": "channel", "type": "text", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("campaign schema creation failed");
    (service, temp_dir)
}

fn campaign(properties: serde_json::Value) -> Node {
    Node::new("campaign".to_string(), "Launch".to_string(), properties)
}

/// A stored campaign whose dates satisfy the rule.
async fn create_valid_campaign(service: &NodeService) -> String {
    service
        .create_node(campaign(json!({
            "start_date": "2026-01-01",
            "end_date": "2026-12-31",
            "channel": "email",
        })))
        .await
        .unwrap()
}

#[derive(Debug, Clone, Copy)]
enum UpdatePath {
    Unchecked,
    UncheckedInTx,
    InTx,
    VersionCheckedInTx,
    Bulk,
}

const UPDATE_PATHS: [UpdatePath; 5] = [
    UpdatePath::Unchecked,
    UpdatePath::UncheckedInTx,
    UpdatePath::InTx,
    UpdatePath::VersionCheckedInTx,
    UpdatePath::Bulk,
];

async fn update_via(
    service: &Arc<NodeService>,
    path: UpdatePath,
    id: &str,
    properties: serde_json::Value,
) -> Result<(), NodeServiceError> {
    apply_via(
        service,
        path,
        id,
        NodeUpdate::new().with_properties(properties),
    )
    .await
}

async fn apply_via(
    service: &Arc<NodeService>,
    path: UpdatePath,
    id: &str,
    update: NodeUpdate,
) -> Result<(), NodeServiceError> {
    let id = id.to_string();
    let in_tx = service.clone();
    match path {
        UpdatePath::Unchecked => service.update_node_unchecked(&id, update).await,
        UpdatePath::Bulk => service.bulk_update(vec![(id, update)]).await,
        UpdatePath::UncheckedInTx => {
            service
                .with_transaction(move |tx| {
                    Box::pin(
                        async move { in_tx.update_node_unchecked_in_tx(tx, &id, update).await },
                    )
                })
                .await
        }
        UpdatePath::InTx => {
            service
                .with_transaction(move |tx| {
                    Box::pin(
                        async move { in_tx.update_node_in_tx(tx, &id, update).await.map(|_| ()) },
                    )
                })
                .await
        }
        UpdatePath::VersionCheckedInTx => {
            let version = service.get_node(&id).await?.unwrap().version;
            service
                .with_transaction(move |tx| {
                    Box::pin(async move {
                        let outcome = in_tx
                            .update_with_version_check_returning_node_in_tx(
                                tx, &id, version, update,
                            )
                            .await?;
                        assert!(
                            matches!(outcome, VersionCheckedUpdateOutcome::Updated { .. }),
                            "the version read just before the update must still match"
                        );
                        Ok(())
                    })
                })
                .await
        }
    }
}

fn assert_refused_by_rule(result: Result<impl std::fmt::Debug, NodeServiceError>, context: &str) {
    match result {
        Err(e) => assert!(
            e.to_string().contains(RULE),
            "{context}: refused, but not by the inherited rule: {e}"
        ),
        Ok(value) => panic!("{context}: the inherited rule was skipped: {value:?}"),
    }
}

/// The dates as stored, read from the declaring type's bucket.
async fn stored_dates(service: &NodeService, id: &str) -> (serde_json::Value, serde_json::Value) {
    let node = service.get_node(id).await.unwrap().unwrap();
    let project = &node.properties["project"];
    (project["start_date"].clone(), project["end_date"].clone())
}

#[tokio::test]
async fn a_flat_update_of_an_inherited_field_is_held_to_the_ancestors_rule() {
    for path in UPDATE_PATHS {
        let (service, _temp) = service_with_campaign_type().await;
        let id = create_valid_campaign(&service).await;

        let result = update_via(&service, path, &id, json!({ "end_date": "2025-06-30" })).await;

        assert_refused_by_rule(result, &format!("{path:?}"));
        assert_eq!(
            stored_dates(&service, &id).await,
            (json!("2026-01-01"), json!("2026-12-31")),
            "{path:?}: a refused update stores nothing"
        );
    }
}

#[tokio::test]
async fn an_update_naming_the_declaring_bucket_is_held_to_the_ancestors_rule() {
    for path in UPDATE_PATHS {
        let (service, _temp) = service_with_campaign_type().await;
        let id = create_valid_campaign(&service).await;

        // The declaring bucket on its own, and beside the node's own bucket.
        for properties in [
            json!({ "project": { "end_date": "2025-06-30" } }),
            json!({ "campaign": {}, "project": { "end_date": "2025-06-30" } }),
        ] {
            let result = update_via(&service, path, &id, properties.clone()).await;

            assert_refused_by_rule(result, &format!("{path:?}, {properties}"));
            assert_eq!(
                stored_dates(&service, &id).await,
                (json!("2026-01-01"), json!("2026-12-31")),
                "{path:?}: a refused update stores nothing"
            );
        }
    }
}

#[tokio::test]
async fn a_valid_node_accepts_a_later_update_that_leaves_the_field_alone() {
    for path in UPDATE_PATHS {
        let (service, _temp) = service_with_campaign_type().await;
        let id = create_valid_campaign(&service).await;

        // An accepted update of the inherited field, in each shape...
        update_via(&service, path, &id, json!({ "end_date": "2026-11-30" }))
            .await
            .unwrap_or_else(|e| panic!("{path:?}: a valid flat update was refused: {e}"));
        update_via(
            &service,
            path,
            &id,
            json!({ "campaign": {}, "project": { "start_date": "2026-01-15" } }),
        )
        .await
        .unwrap_or_else(|e| panic!("{path:?}: a valid bucketed update was refused: {e}"));
        update_via(
            &service,
            path,
            &id,
            json!({ "project": { "start_date": "2026-02-01" } }),
        )
        .await
        .unwrap_or_else(|e| panic!("{path:?}: a valid declaring-bucket update was refused: {e}"));

        // ...then updates that do not touch it.
        update_via(&service, path, &id, json!({ "channel": "social" }))
            .await
            .unwrap_or_else(|e| panic!("{path:?}: an own-field update was refused: {e}"));
        apply_via(
            &service,
            path,
            &id,
            NodeUpdate::new().with_content("Relaunch".to_string()),
        )
        .await
        .unwrap_or_else(|e| panic!("{path:?}: a content update was refused: {e}"));

        let node = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(
            node.properties,
            json!({
                "project": {
                    "status": "planning",
                    "start_date": "2026-02-01",
                    "end_date": "2026-11-30",
                },
                "campaign": { "channel": "social" },
            }),
            "{path:?}: each field is stored once, in its declaring bucket"
        );
    }
}

/// The create shapes that put an inherited field somewhere other than its
/// declaring bucket, plus the declaring bucket itself.
fn rejected_create_shapes() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        (
            "flat",
            json!({ "start_date": "2026-12-31", "end_date": "2026-01-01" }),
        ),
        (
            "own bucket",
            json!({ "campaign": { "start_date": "2026-12-31", "end_date": "2026-01-01" } }),
        ),
        (
            "declaring bucket",
            json!({ "project": { "start_date": "2026-12-31", "end_date": "2026-01-01" } }),
        ),
        (
            "declaring bucket beside the own bucket",
            json!({
                "campaign": {},
                "project": { "start_date": "2026-12-31", "end_date": "2026-01-01" }
            }),
        ),
    ]
}

#[tokio::test]
async fn a_single_create_is_held_to_the_ancestors_rule() {
    let (service, _temp) = service_with_campaign_type().await;
    for (shape, properties) in rejected_create_shapes() {
        let result = service.create_node(campaign(properties)).await;
        assert_refused_by_rule(result, shape);
    }
}

#[tokio::test]
async fn a_bulk_create_is_held_to_the_ancestors_rule() {
    let (service, _temp) = service_with_campaign_type().await;
    for (shape, properties) in rejected_create_shapes() {
        let result = service.bulk_create(vec![campaign(properties)]).await;
        assert_refused_by_rule(result, shape);
    }
}

#[tokio::test]
async fn a_hierarchy_import_is_held_to_the_ancestors_rule() {
    let (service, _temp) = service_with_campaign_type().await;
    let row = |properties: serde_json::Value| {
        vec![(
            uuid::Uuid::new_v4().to_string(),
            "campaign".to_string(),
            "Launch".to_string(),
            None,
            1.0,
            properties,
        )]
    };
    for (shape, properties) in rejected_create_shapes() {
        let result = service.bulk_create_hierarchy(row(properties.clone())).await;
        assert_refused_by_rule(result, &format!("bulk_create_hierarchy, {shape}"));

        let result = service.bulk_create_hierarchy_trusted(row(properties)).await;
        assert_refused_by_rule(result, &format!("bulk_create_hierarchy_trusted, {shape}"));
    }
}

/// A trusted import stores an inherited field in its declaring bucket, as
/// every other create does: the bucket the ancestor's behaviour reads.
#[tokio::test]
async fn a_trusted_import_stores_an_inherited_field_in_its_declaring_bucket() {
    let (service, _temp) = service_with_campaign_type().await;
    let id = uuid::Uuid::new_v4().to_string();
    service
        .bulk_create_hierarchy_trusted(vec![(
            id.clone(),
            "campaign".to_string(),
            "Launch".to_string(),
            None,
            1.0,
            json!({ "start_date": "2026-01-01", "end_date": "2026-12-31", "channel": "email" }),
        )])
        .await
        .unwrap();

    let node = service.get_node(&id).await.unwrap().unwrap();
    assert_eq!(
        node.properties,
        json!({
            "project": { "start_date": "2026-01-01", "end_date": "2026-12-31" },
            "campaign": { "channel": "email" },
        })
    );
}

/// A retype defaults the new type's fields, inherited ones included, before
/// anything validates the node: the default lands in its declaring bucket,
/// and the ancestor's behaviour sees the node as it will be stored.
#[tokio::test]
async fn a_retype_into_a_subtype_defaults_inherited_fields_before_validating() {
    for path in UPDATE_PATHS {
        // The version-checked update never defaults: it is not a retype path.
        if matches!(path, UpdatePath::VersionCheckedInTx) {
            continue;
        }
        let (service, _temp) = service_with_campaign_type().await;
        let id = service
            .create_node(Node::new(
                "text".to_string(),
                "Launch".to_string(),
                json!({}),
            ))
            .await
            .unwrap();

        apply_via(
            &service,
            path,
            &id,
            NodeUpdate::new().with_node_type("campaign".to_string()),
        )
        .await
        .unwrap_or_else(|e| panic!("{path:?}: the retype was refused: {e}"));

        let node = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(node.node_type, "campaign", "{path:?}");
        assert_eq!(
            node.properties["project"]["status"],
            json!("planning"),
            "{path:?}: the inherited default is stored in its declaring bucket: {}",
            node.properties
        );
    }
}

/// A value named in its declaring bucket is the value stored: a create or a
/// retype does not default over it, and it wins over the same field given
/// flat in the same write.
#[tokio::test]
async fn a_value_named_in_its_declaring_bucket_is_not_defaulted_over() {
    let (service, _temp) = service_with_campaign_type().await;
    let expected = json!({
        "project": { "status": "active", "end_date": "2026-12-31" },
        "campaign": {},
    });
    // The addressed value alone, where a default would otherwise apply, and
    // beside the same field given flat.
    let alone = json!({ "project": { "status": "active", "end_date": "2026-12-31" } });
    let addressed = json!({
        "status": "completed",
        "project": { "status": "active", "end_date": "2026-12-31" },
    });

    for properties in [&alone, &addressed] {
        let id = service
            .create_node(campaign(properties.clone()))
            .await
            .unwrap();
        let created = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(created.properties, expected, "create with {properties}");
    }

    for path in UPDATE_PATHS {
        // The version-checked update never defaults: it is not a retype path.
        if matches!(path, UpdatePath::VersionCheckedInTx) {
            continue;
        }
        let id = service
            .create_node(Node::new(
                "text".to_string(),
                "Launch".to_string(),
                json!({}),
            ))
            .await
            .unwrap();
        apply_via(
            &service,
            path,
            &id,
            NodeUpdate::new()
                .with_node_type("campaign".to_string())
                .with_properties(alone.clone()),
        )
        .await
        .unwrap_or_else(|e| panic!("{path:?}: the retype was refused: {e}"));

        let node = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(
            node.properties["project"], expected["project"],
            "{path:?}: retype"
        );
    }
}

/// An entry of a named bucket goes to the schema that declares the field,
/// whichever ancestor the caller named, so the behaviour that owns the rule
/// reads it.
#[tokio::test]
async fn an_entry_of_a_named_bucket_is_stored_where_its_field_is_declared() {
    let (service, _temp) = service_with_campaign_type().await;
    crate::schema::handle_create_schema(
        &service,
        json!({
            "name": "Promo",
            "extends": "campaign",
            "fields": [
                { "name": "code", "type": "text", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("promo schema creation failed");
    let promo = |properties: serde_json::Value| {
        Node::new("promo".to_string(), "Sale".to_string(), properties)
    };

    // `project`'s dates, named in the `campaign` bucket.
    let result = service
        .create_node(promo(json!({
            "campaign": { "start_date": "2026-12-31", "end_date": "2026-01-01" }
        })))
        .await;
    assert_refused_by_rule(result, "a nearer ancestor's bucket");

    let id = service
        .create_node(promo(json!({
            "campaign": { "end_date": "2026-12-31", "channel": "email", "code": "SALE" }
        })))
        .await
        .unwrap();
    let node = service.get_node(&id).await.unwrap().unwrap();
    assert_eq!(
        node.properties,
        json!({
            "project": { "status": "planning", "end_date": "2026-12-31" },
            "campaign": { "channel": "email" },
            "promo": { "code": "SALE" },
        })
    );
}

/// Properties that are not an object, or that hold something other than an
/// object where a type's bucket is stored, are refused rather than replaced
/// by defaults or dropped.
#[tokio::test]
async fn properties_that_are_not_bucketed_are_refused() {
    let (service, _temp) = service_with_campaign_type().await;

    for node_type in ["campaign", "task"] {
        let result = service
            .create_node(Node::new(
                node_type.to_string(),
                "Launch".to_string(),
                json!("not an object"),
            ))
            .await;
        let error = result.expect_err("the properties were discarded instead of refused");
        assert!(
            error
                .to_string()
                .contains("Properties must be a JSON object"),
            "{node_type}: {error}"
        );
    }

    // The declaring bucket holds a string: the dates addressed to it have
    // nowhere to go.
    let result = service
        .bulk_create(vec![campaign(json!({
            "campaign": { "project": { "end_date": "2026-12-31" } },
            "project": "oops",
        }))])
        .await;
    let error = result.expect_err("the addressed dates were dropped instead of refused");
    assert!(
        error.to_string().contains("'project' in properties"),
        "{error}"
    );
}
