//! The core type registry's rules, end to end against a real store
//! (ADR-086 §5–§7a, §10): a subtype takes its base type's rules, an abstract
//! type is never instantiated, a core type's bucket is closed, a typed
//! client's generic update leaves typed core fields alone, the field-type
//! vocabulary is closed, and a node id is a UUID outside three named forms.

use nodespace_core::db::{SqliteStore, TreeInvariantRule};
use nodespace_core::models::{CoreNodeType, Node, NodeUpdate};
use nodespace_core::schema::{handle_create_schema, handle_update_schema};
use nodespace_core::services::{
    CreateNodeParams, InsertPositionOwned, NodeService, NodeServiceError,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn test_service() -> (Arc<NodeService>, TempDir) {
    let temp_dir = TempDir::new().expect("tempdir creation failed");
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(
        SqliteStore::new(db_path)
            .await
            .expect("SqliteStore init failed"),
    );
    let node_service = Arc::new(
        NodeService::new(&mut store)
            .await
            .expect("NodeService init failed"),
    );
    (node_service, temp_dir)
}

async fn create_schema(svc: &Arc<NodeService>, params: serde_json::Value) {
    handle_create_schema(svc, params.clone())
        .await
        .unwrap_or_else(|e| panic!("creating schema {params} failed: {e:?}"));
}

async fn create(
    svc: &Arc<NodeService>,
    node_type: &str,
    content: &str,
    properties: serde_json::Value,
) -> Result<String, NodeServiceError> {
    svc.create_node(Node::new(
        node_type.to_string(),
        content.to_string(),
        properties,
    ))
    .await
}

async fn create_under(
    svc: &Arc<NodeService>,
    parent_id: &str,
    node_type: &str,
    content: &str,
) -> Result<String, NodeServiceError> {
    svc.create_node_with_parent(CreateNodeParams {
        id: None,
        node_type: node_type.to_string(),
        content: content.to_string(),
        parent_id: Some(parent_id.to_string()),
        position: InsertPositionOwned::End,
        properties: json!({}),
        lifecycle_status: None,
    })
    .await
}

fn message(result: Result<impl std::fmt::Debug, NodeServiceError>) -> String {
    result.expect_err("the write must be refused").to_string()
}

// ---------------------------------------------------------------------------
// Rules resolve through the `extends` chain
// ---------------------------------------------------------------------------

/// `issue extends task`, a user-defined subtype of a core type.
async fn seed_issue(svc: &Arc<NodeService>) {
    create_schema(
        svc,
        json!({
            "name": "Issue",
            "extends": "task",
            "fields": [{ "name": "severity", "type": "text" }]
        }),
    )
    .await;
}

#[tokio::test]
async fn a_user_subtype_resolves_to_its_core_type() {
    let (svc, _tmp) = test_service().await;
    seed_issue(&svc).await;

    assert_eq!(
        svc.store().type_chain("issue").await.unwrap(),
        vec!["issue", "task"]
    );
    assert_eq!(
        svc.core_type_of("issue").await.unwrap(),
        Some(CoreNodeType::Task)
    );
    assert!(svc.type_is_a("issue", CoreNodeType::Task).await.unwrap());
    assert!(!svc.type_is_a("issue", CoreNodeType::Project).await.unwrap());
    // The ancestry table agrees with the `extends` edges it is derived from.
    assert_eq!(
        svc.store()
            .get_extends_parent_map()
            .await
            .unwrap()
            .get("issue"),
        Some(&"task".to_string())
    );
    // A type with no schema is its own chain, and no core type.
    assert_eq!(svc.core_type_of("nothing").await.unwrap(), None);
}

/// A subtype of `task` is validated as a task: the inherited `status` is held
/// to the task schema's vocabulary, defaulted like a task's, and stored in the
/// task bucket.
#[tokio::test]
async fn a_subtype_of_task_is_validated_as_a_task() {
    let (svc, _tmp) = test_service().await;
    seed_issue(&svc).await;

    let error = message(create(&svc, "issue", "Bad", json!({ "status": "nonsense" })).await);
    assert!(error.contains("status"), "{error}");

    let id = create(&svc, "issue", "Good", json!({ "severity": "high" }))
        .await
        .unwrap();
    let node = svc.get_node(&id).await.unwrap().unwrap();
    assert_eq!(node.node_type, "issue");
    assert_eq!(node.properties["task"]["status"], "open");
    assert_eq!(node.properties["issue"]["severity"], "high");
}

/// A subtype of `task` is embedded as a task is — not at all — and is titled
/// at any depth, as a task is.
#[tokio::test]
async fn a_subtype_of_task_is_embedded_and_titled_as_a_task() {
    let (svc, _tmp) = test_service().await;
    seed_issue(&svc).await;

    let page = create(&svc, "text", "A page", json!({})).await.unwrap();
    let root_issue = create(&svc, "issue", "A root issue", json!({}))
        .await
        .unwrap();
    let child_issue = create_under(&svc, &page, "issue", "A child issue")
        .await
        .unwrap();
    let child_text = create_under(&svc, &page, "text", "A child line")
        .await
        .unwrap();

    let stale = svc
        .store()
        .get_stale_embedding_root_ids(None, 0, u8::MAX)
        .await
        .unwrap();
    assert!(stale.contains(&page), "a text root is queued for embedding");
    assert!(
        !stale.contains(&root_issue),
        "a root issue is no more embedded than a root task"
    );

    let title = |id: String| {
        let svc = svc.clone();
        async move { svc.get_node(&id).await.unwrap().unwrap().title }
    };
    assert_eq!(title(child_issue).await.as_deref(), Some("A child issue"));
    assert_eq!(
        title(child_text).await,
        None,
        "an ordinary child line has no title"
    );
}

/// A subtype of `collection` is a collection wherever one is asked for: it is
/// root-only on every path, keeps the collection's naming rule, can be a
/// `member_of` target, and stays out of the `@` picker.
#[tokio::test]
async fn a_subtype_of_collection_is_root_only_and_keeps_the_collections_rules() {
    let (svc, _tmp) = test_service().await;
    create_schema(
        &svc,
        json!({ "name": "Team", "extends": "collection", "fields": [] }),
    )
    .await;

    let page = create(&svc, "text", "A page", json!({})).await.unwrap();
    let team = create(&svc, "team", "Core team", json!({})).await.unwrap();

    // Created under a parent.
    let refused = create_under(&svc, &page, "team", "Nested team")
        .await
        .expect_err("a team is a collection, and a collection is a root");
    match refused {
        NodeServiceError::TreeInvariantViolation(v) => {
            assert_eq!(v.rule, TreeInvariantRule::MustBeRoot);
        }
        other => panic!("expected a tree invariant violation, got {other:?}"),
    }

    // Given a parent by a `has_child` edge.
    let error = message(
        svc.create_relationship(&page, "has_child", &team, json!({}))
            .await,
    );
    assert!(error.contains("must_be_root"), "{error}");

    // Retyped into while it has a parent.
    let line = create_under(&svc, &page, "text", "A line").await.unwrap();
    let version = svc.get_node(&line).await.unwrap().unwrap().version;
    let error = message(
        svc.update_node(
            &line,
            version,
            NodeUpdate::new().with_node_type("team".to_string()),
        )
        .await,
    );
    assert!(error.contains("must_be_root"), "{error}");

    // The collection's naming rule.
    let error = message(create(&svc, "team", "a:b", json!({})).await);
    assert!(error.contains(':'), "{error}");

    // A team is a valid `member_of` target, like any collection.
    svc.create_relationship(&page, "member_of", &team, json!({}))
        .await
        .expect("a root page may be filed into a team");

    // Unmentionable, like any collection; a page is offered.
    let offered: Vec<String> = svc
        .store()
        .mention_autocomplete("", None)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert!(offered.contains(&page));
    assert!(!offered.contains(&team), "a team is not a mention target");
}

// ---------------------------------------------------------------------------
// Abstract types
// ---------------------------------------------------------------------------

/// `vehicle` is abstract; `car extends vehicle` is not.
async fn seed_vehicle(svc: &Arc<NodeService>) {
    create_schema(
        svc,
        json!({
            "name": "Vehicle",
            "abstract": true,
            "fields": [{ "name": "wheels", "type": "number" }]
        }),
    )
    .await;
    create_schema(
        svc,
        json!({ "name": "Car", "extends": "vehicle", "fields": [] }),
    )
    .await;
}

#[tokio::test]
async fn an_abstract_type_is_refused_on_every_create_path() {
    let (svc, _tmp) = test_service().await;
    seed_vehicle(&svc).await;
    let abstract_error = |e: String| assert!(e.contains("abstract type"), "{e}");

    abstract_error(message(
        create(&svc, "vehicle", "A vehicle", json!({})).await,
    ));

    let page = create(&svc, "text", "A page", json!({})).await.unwrap();
    abstract_error(message(
        create_under(&svc, &page, "vehicle", "A vehicle").await,
    ));

    abstract_error(message(
        svc.bulk_create(vec![Node::new(
            "vehicle".to_string(),
            "A vehicle".to_string(),
            json!({}),
        )])
        .await,
    ));

    abstract_error(message(
        svc.bulk_create_hierarchy(vec![(
            uuid::Uuid::new_v4().to_string(),
            "vehicle".to_string(),
            "A vehicle".to_string(),
            None,
            0.0,
            json!({}),
        )])
        .await,
    ));

    // Its subtype is created normally and carries the base's fields.
    let car = create(&svc, "car", "A car", json!({ "wheels": 4 }))
        .await
        .unwrap();
    let node = svc.get_node(&car).await.unwrap().unwrap();
    assert_eq!(node.properties["vehicle"]["wheels"], 4);
}

#[tokio::test]
async fn retyping_into_an_abstract_type_is_refused() {
    let (svc, _tmp) = test_service().await;
    seed_vehicle(&svc).await;

    let id = create(&svc, "car", "A car", json!({})).await.unwrap();
    let version = svc.get_node(&id).await.unwrap().unwrap().version;
    let error = message(
        svc.update_node(
            &id,
            version,
            NodeUpdate::new().with_node_type("vehicle".to_string()),
        )
        .await,
    );
    assert!(error.contains("abstract type"), "{error}");
    assert_eq!(svc.get_node(&id).await.unwrap().unwrap().node_type, "car");
}

/// An abstract type is still a real type: it is a query scope, and a query
/// for it returns its subtypes' nodes (no node has the abstract type itself).
#[tokio::test]
async fn a_query_for_an_abstract_type_returns_its_subtypes() {
    let (svc, _tmp) = test_service().await;
    seed_vehicle(&svc).await;
    let car = create(&svc, "car", "A car", json!({})).await.unwrap();

    let found = svc.query_nodes_by_type("vehicle", true).await.unwrap();
    assert_eq!(
        found.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
        vec![car.as_str()]
    );
    assert!(found.iter().all(|n| n.node_type == "car"));
}

/// A type can be made abstract later, but not while nodes of its own exist.
#[tokio::test]
async fn a_type_with_instances_cannot_become_abstract() {
    let (svc, _tmp) = test_service().await;
    create_schema(&svc, json!({ "name": "Gadget", "fields": [] })).await;
    create_schema(&svc, json!({ "name": "Widget", "fields": [] })).await;
    create(&svc, "gadget", "A gadget", json!({})).await.unwrap();

    let error = handle_update_schema(&svc, json!({ "schema_id": "gadget", "abstract": true }))
        .await
        .expect_err("gadget has nodes of its own")
        .to_string();
    assert!(error.contains("cannot become abstract"), "{error}");

    handle_update_schema(&svc, json!({ "schema_id": "widget", "abstract": true }))
        .await
        .expect("widget has no nodes");
    let error = message(create(&svc, "widget", "A widget", json!({})).await);
    assert!(error.contains("abstract type"), "{error}");

    // An unrelated update leaves the flag as it was.
    handle_update_schema(
        &svc,
        json!({ "schema_id": "widget", "add_fields": [{ "name": "size", "type": "number" }] }),
    )
    .await
    .expect("adding a field");
    assert!(svc.store().is_abstract_type("widget").await.unwrap());
}

// ---------------------------------------------------------------------------
// Closed core schemas
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_undeclared_key_in_a_core_bucket_is_rejected() {
    let (svc, _tmp) = test_service().await;

    let error = message(create(&svc, "task", "A task", json!({ "colour": "red" })).await);
    assert!(
        error.contains("'colour' is not a field of the core type 'task'"),
        "{error}"
    );

    // A primitive declares no fields, so its bucket takes no bare key at all.
    let error = message(create(&svc, "text", "A line", json!({ "colour": "red" })).await);
    assert!(
        error.contains("'colour' is not a field of the core type 'text'"),
        "{error}"
    );

    // On an update as on a create.
    let id = create(&svc, "task", "A task", json!({})).await.unwrap();
    let version = svc.get_node(&id).await.unwrap().unwrap().version;
    let error = message(
        svc.update_node(
            &id,
            version,
            NodeUpdate::new().with_properties(json!({ "colour": "red" })),
        )
        .await,
    );
    assert!(error.contains("'colour' is not a field"), "{error}");
}

#[tokio::test]
async fn a_namespaced_key_and_a_bookkeeping_key_are_accepted() {
    let (svc, _tmp) = test_service().await;

    let id = create(
        &svc,
        "task",
        "A task",
        json!({
            "status": "open",
            "custom:colour": "red",
            "org:cost_center": "42",
            "plugin:external_id": "x-1",
            "_seed": { "tier": "core" }
        }),
    )
    .await
    .expect("declared, namespaced and bookkeeping keys are all allowed");

    let node = svc.get_node(&id).await.unwrap().unwrap();
    assert_eq!(node.properties["task"]["custom:colour"], "red");
    assert_eq!(node.properties["task"]["org:cost_center"], "42");
    assert_eq!(node.properties["task"]["plugin:external_id"], "x-1");
    assert_eq!(node.properties["_seed"]["tier"], "core");

    // A prefix alone, or an unknown one, is not an extension field.
    for key in ["custom:", "vendor:thing"] {
        let error = message(create(&svc, "task", "A task", json!({ key: "x" })).await);
        assert!(error.contains("is not a field of the core type"), "{error}");
    }
}

/// The rule is the core type's bucket's. A subtype's own bucket follows its
/// own schema, and a user-defined type stays open.
#[tokio::test]
async fn a_subtype_bucket_and_a_user_type_stay_open() {
    let (svc, _tmp) = test_service().await;
    seed_issue(&svc).await;
    create_schema(&svc, json!({ "name": "Invoice", "fields": [] })).await;

    let issue = create(&svc, "issue", "An issue", json!({ "anything": 1 }))
        .await
        .expect("the issue bucket is the user's");
    let node = svc.get_node(&issue).await.unwrap().unwrap();
    assert_eq!(node.properties["issue"]["anything"], 1);
    assert!(node.properties["task"].get("anything").is_none());

    create(&svc, "invoice", "An invoice", json!({ "anything": 1 }))
        .await
        .expect("a user-defined type is open");
}

/// Nested declarations are validated at every depth: `fields` of an object
/// and `itemFields` of an array of objects get the same type, enum and
/// required checks as a top-level field.
#[tokio::test]
async fn nested_fields_and_item_fields_are_validated_recursively() {
    let (svc, _tmp) = test_service().await;
    create_schema(
        &svc,
        json!({
            "name": "Order",
            "fields": [
                {
                    "name": "shipping",
                    "type": "object",
                    "fields": [
                        { "name": "city", "type": "text", "required": true },
                        {
                            "name": "box",
                            "type": "object",
                            "fields": [{ "name": "weight", "type": "number" }]
                        }
                    ]
                },
                {
                    "name": "lines",
                    "type": "array",
                    "itemType": "object",
                    "itemFields": [
                        { "name": "sku", "type": "text", "required": true },
                        { "name": "quantity", "type": "number" },
                        {
                            "name": "state",
                            "type": "enum",
                            "coreValues": [
                                { "value": "new", "label": "New" },
                                { "value": "sent", "label": "Sent" }
                            ]
                        }
                    ]
                }
            ]
        }),
    )
    .await;

    create(
        &svc,
        "order",
        "A valid order",
        json!({
            "shipping": { "city": "Oslo", "box": { "weight": 2.5 } },
            "lines": [{ "sku": "a-1", "quantity": 2, "state": "new" }]
        }),
    )
    .await
    .expect("a well-formed order");

    let cases = [
        (json!({ "shipping": {} }), "city"),
        (
            json!({ "shipping": { "city": "Oslo", "box": { "weight": "heavy" } } }),
            "weight",
        ),
        (json!({ "lines": [{ "quantity": 2 }] }), "sku"),
        (
            json!({ "lines": [{ "sku": "a-1", "quantity": "two" }] }),
            "quantity",
        ),
        (
            json!({ "lines": [{ "sku": "a-1" }, { "sku": "a-2", "state": "lost" }] }),
            "state",
        ),
    ];
    for (properties, field) in cases {
        let error = message(create(&svc, "order", "A bad order", properties.clone()).await);
        assert!(
            error.contains(field),
            "{properties} must be rejected naming '{field}', got: {error}"
        );
    }
}

// ---------------------------------------------------------------------------
// Typed clients
// ---------------------------------------------------------------------------

/// A typed client's generic update may not name a core field that has a typed
/// update; the same patch from a client that writes bare keys (the CLI, the
/// agent) goes through the shared pipeline and is validated there.
#[tokio::test]
async fn a_typed_clients_generic_update_may_not_name_a_typed_core_field() {
    let (svc, _tmp) = test_service().await;
    seed_issue(&svc).await;
    let task = create(&svc, "task", "A task", json!({})).await.unwrap();

    for patch in [
        json!({ "status": "done" }),
        json!({ "dueDate": "2026-01-01" }),
        json!({ "due_date": "2026-01-01" }),
        json!({ "task": { "status": "done" } }),
    ] {
        let error = message(svc.ensure_no_typed_core_fields(&task, None, &patch).await);
        assert!(
            error.contains("typed task update"),
            "{patch} must be refused, got: {error}"
        );
    }

    // What the generic update is still for: extension fields.
    svc.ensure_no_typed_core_fields(&task, None, &json!({ "custom:colour": "red" }))
        .await
        .expect("an extension field has no typed path");

    // A retype is read against the type the node will have.
    let line = create(&svc, "text", "A line", json!({})).await.unwrap();
    let error = message(
        svc.ensure_no_typed_core_fields(&line, Some("task"), &json!({ "status": "done" }))
            .await,
    );
    assert!(error.contains("typed task update"), "{error}");

    // A subtype travels as a generic node: its inherited fields have no typed
    // update to prefer.
    let issue = create(&svc, "issue", "An issue", json!({})).await.unwrap();
    svc.ensure_no_typed_core_fields(&issue, None, &json!({ "status": "done" }))
        .await
        .expect("a subtype is written through the generic update");

    // A core type with no typed update is written through the generic one.
    let collection = create(&svc, "collection", "notes", json!({}))
        .await
        .unwrap();
    svc.ensure_no_typed_core_fields(&collection, None, &json!({ "description": "My notes" }))
        .await
        .expect("collection has no typed update");

    // The flat update itself — the CLI's `node update --property status=done`
    // — writes the same field through the validated pipeline.
    let version = svc.get_node(&task).await.unwrap().unwrap().version;
    let updated = svc
        .update_node(
            &task,
            version,
            NodeUpdate::new().with_properties(json!({ "status": "done" })),
        )
        .await
        .expect("a bare-key update of a core field is validated and written");
    assert_eq!(updated.properties["task"]["status"], "done");
    let error = message(
        svc.update_node(
            &task,
            updated.version,
            NodeUpdate::new().with_properties(json!({ "status": "nonsense" })),
        )
        .await,
    );
    assert!(error.contains("status"), "{error}");
}

// ---------------------------------------------------------------------------
// One field-type vocabulary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_is_not_a_field_type() {
    let (svc, _tmp) = test_service().await;

    for field in [
        json!({ "name": "title", "type": "string" }),
        json!({ "name": "tags", "type": "array", "itemType": "string" }),
        json!({ "name": "title", "type": "varchar" }),
    ] {
        let error = handle_create_schema(&svc, json!({ "name": "Doc", "fields": [field.clone()] }))
            .await
            .expect_err("an unknown field type must be refused")
            .to_string();
        assert!(
            error.contains("field type"),
            "{field} must be refused, got: {error}"
        );
    }

    // The message for `string` names the type to use.
    let error = handle_create_schema(
        &svc,
        json!({ "name": "Doc", "fields": [{ "name": "title", "type": "string" }] }),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("use 'text'"), "{error}");

    create_schema(
        &svc,
        json!({
            "name": "Doc",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "tags", "type": "array", "itemType": "text" }
            ]
        }),
    )
    .await;
}

// ---------------------------------------------------------------------------
// Ids
// ---------------------------------------------------------------------------

fn with_id(id: &str, node_type: &str) -> Node {
    Node::new_with_id(
        id.to_string(),
        node_type.to_string(),
        "content".to_string(),
        json!({}),
    )
}

#[tokio::test]
async fn a_uuid_is_accepted_and_a_slug_is_rejected() {
    let (svc, _tmp) = test_service().await;

    svc.create_node(with_id("0d0c7e3a-6a55-4f1b-9d0f-0b9f6f2a8c11", "text"))
        .await
        .expect("a UUID");

    for slug in ["my-note", "note_1", "42", ""] {
        let error = message(svc.create_node(with_id(slug, "text")).await);
        assert!(
            error.contains("not a valid UUID") || error.contains("id"),
            "'{slug}' must be rejected, got: {error}"
        );
    }

    // The bulk paths hold the same rule.
    let error = message(svc.bulk_create(vec![with_id("my-note", "text")]).await);
    assert!(error.contains("not a valid UUID"), "{error}");
    let error = message(
        svc.bulk_create_hierarchy(vec![(
            "my-note".to_string(),
            "text".to_string(),
            "content".to_string(),
            None,
            0.0,
            json!({}),
        )])
        .await,
    );
    assert!(error.contains("not a valid UUID"), "{error}");
}

#[tokio::test]
async fn a_date_id_is_accepted_for_a_date_node() {
    let (svc, _tmp) = test_service().await;

    // A date-shaped id makes the node a date page, whatever type was asked.
    let id = svc
        .create_node(with_id("2026-10-01", "text"))
        .await
        .expect("a date page");
    assert_eq!(id, "2026-10-01");
    assert_eq!(svc.get_node(&id).await.unwrap().unwrap().node_type, "date");

    // A date node whose id is not a real date has no legal id form.
    let error = message(svc.create_node(with_id("2026-13-45", "date")).await);
    assert!(error.contains("not a valid UUID"), "{error}");
}

#[tokio::test]
async fn a_type_name_id_is_accepted_for_a_schema_node() {
    let (svc, _tmp) = test_service().await;

    create_schema(&svc, json!({ "name": "Customer Profile", "fields": [] })).await;
    assert!(svc
        .get_schema_node("customer_profile")
        .await
        .unwrap()
        .is_some());

    // The type-name form is the schema's alone.
    let error = message(svc.create_node(with_id("customer_profile", "text")).await);
    assert!(error.contains("not a valid UUID"), "{error}");
}

#[tokio::test]
async fn the_settings_singleton_keeps_its_fixed_id() {
    let (svc, _tmp) = test_service().await;
    const SETTINGS_ID: &str = "database-settings-singleton";

    let settings = svc
        .get_node(SETTINGS_ID)
        .await
        .unwrap()
        .expect("the settings singleton is seeded under its fixed id");
    assert_eq!(settings.node_type, "database-settings");

    // The fixed id is the singleton's alone.
    let error = message(svc.create_node(with_id(SETTINGS_ID, "text")).await);
    assert!(error.contains("not a valid UUID"), "{error}");
}

// ---------------------------------------------------------------------------
// The registry is the authority for core types
// ---------------------------------------------------------------------------

/// The ancestry table follows the schema API: a re-target moves the chain,
/// and deleting the schema removes the type.
#[tokio::test]
async fn the_type_chain_follows_a_retarget_and_a_schema_delete() {
    let (svc, _tmp) = test_service().await;
    create_schema(&svc, json!({ "name": "Asset", "fields": [] })).await;
    create_schema(&svc, json!({ "name": "Device", "fields": [] })).await;
    create_schema(
        &svc,
        json!({ "name": "Laptop", "extends": "asset", "fields": [] }),
    )
    .await;
    assert_eq!(
        svc.store().type_chain("laptop").await.unwrap(),
        vec!["laptop", "asset"]
    );

    handle_update_schema(&svc, json!({ "schema_id": "laptop", "extends": "device" }))
        .await
        .expect("re-target");
    assert_eq!(
        svc.store().type_chain("laptop").await.unwrap(),
        vec!["laptop", "device"]
    );
    assert_eq!(
        svc.resolve_type_chain("laptop").await.unwrap(),
        vec!["laptop", "device"]
    );

    let schema = svc.get_node("laptop").await.unwrap().unwrap();
    svc.delete_node("laptop", schema.version)
        .await
        .expect("delete the schema");
    assert_eq!(
        svc.store().type_chain("laptop").await.unwrap(),
        vec!["laptop"],
        "a type with no schema is its own chain"
    );
    assert_eq!(
        svc.store().type_chain("device").await.unwrap(),
        vec!["device"]
    );
}

/// A user schema cannot take a core type's name, the meta-type's included,
/// which has no schema node of its own to collide with.
#[tokio::test]
async fn a_schema_cannot_take_a_core_types_name() {
    let (svc, _tmp) = test_service().await;
    for name in ["Schema", "Task", "Collection"] {
        let error = handle_create_schema(&svc, json!({ "name": name, "fields": [] }))
            .await
            .expect_err("a core type's name is taken")
            .to_string();
        assert!(error.contains("core type"), "{name}: {error}");
    }
}

/// What a core type extends and whether it is abstract are the registry's;
/// the schema API cannot change either, so SQL and Rust cannot disagree.
#[tokio::test]
async fn a_core_types_parent_and_abstract_flag_are_fixed() {
    let (svc, _tmp) = test_service().await;
    for params in [
        json!({ "schema_id": "text", "extends": "collection" }),
        json!({ "schema_id": "query", "abstract": true }),
    ] {
        let error = handle_update_schema(&svc, params.clone())
            .await
            .expect_err("a core type's place in the type system is fixed")
            .to_string();
        assert!(error.contains("core type"), "{params}: {error}");
    }
    assert_eq!(svc.store().type_chain("text").await.unwrap(), vec!["text"]);
    assert!(!svc.store().is_abstract_type("query").await.unwrap());
    create(&svc, "text", "Still a plain text node", json!({}))
        .await
        .expect("text is concrete and extends nothing");
}

/// Taking on a root-only base tightens the rule for the type's existing
/// nodes, so it is refused while one of them has a parent.
#[tokio::test]
async fn a_type_with_parented_nodes_cannot_take_a_root_only_base() {
    let (svc, _tmp) = test_service().await;
    create_schema(&svc, json!({ "name": "Folder", "fields": [] })).await;
    create_schema(&svc, json!({ "name": "Shelf", "fields": [] })).await;

    let page = create(&svc, "text", "A page", json!({})).await.unwrap();
    create_under(&svc, &page, "folder", "nested").await.unwrap();
    create(&svc, "shelf", "top-level", json!({})).await.unwrap();

    let error = handle_update_schema(
        &svc,
        json!({ "schema_id": "folder", "extends": "collection" }),
    )
    .await
    .expect_err("a nested folder would become a collection with a parent")
    .to_string();
    assert!(error.contains("has a parent"), "{error}");
    assert_eq!(
        svc.store().type_chain("folder").await.unwrap(),
        vec!["folder"]
    );

    handle_update_schema(
        &svc,
        json!({ "schema_id": "shelf", "extends": "collection" }),
    )
    .await
    .expect("every shelf is a root");
    assert!(svc
        .type_is_a("shelf", CoreNodeType::Collection)
        .await
        .unwrap());
}

/// A relationship may not target a chat, and that holds for a type extending
/// `ai-chat` as for `ai-chat` itself; a mention of one is dropped by the bulk
/// path exactly as the single-edge path refuses it.
#[tokio::test]
async fn a_subtype_of_ai_chat_cannot_be_referenced() {
    let (svc, _tmp) = test_service().await;
    create_schema(
        &svc,
        json!({ "name": "Support Chat", "extends": "ai-chat", "fields": [] }),
    )
    .await;

    let error = handle_create_schema(
        &svc,
        json!({
            "name": "Ticket",
            "fields": [],
            "relationships": [{
                "name": "discussed_in",
                "targetType": "support_chat",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "tickets",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .expect_err("a relationship may not target a chat subtype")
    .to_string();
    assert!(error.contains("AI chat"), "{error}");

    let chat = create(
        &svc,
        "support_chat",
        "Help",
        json!({ "agent": "nodespace" }),
    )
    .await
    .unwrap();
    let page = create(&svc, "text", "A page", json!({})).await.unwrap();
    let other = create(&svc, "text", "Another page", json!({}))
        .await
        .unwrap();

    let error = message(svc.create_mention(&page, &chat).await);
    assert!(error.contains("ai-chat"), "{error}");

    let created = svc
        .store()
        .bulk_create_mentions(&[(page.clone(), chat.clone()), (page.clone(), other.clone())])
        .await
        .unwrap();
    assert_eq!(created, 1, "the mention of the chat is dropped");
}

/// A refused type-system change leaves the whole call unapplied: it is
/// decided before a rename in the same call commits and migrates node data.
#[tokio::test]
async fn a_refused_type_system_change_applies_nothing_else_in_the_call() {
    let (svc, _tmp) = test_service().await;
    create_schema(
        &svc,
        json!({ "name": "Folder", "fields": [{ "name": "colour", "type": "text" }] }),
    )
    .await;
    let page = create(&svc, "text", "A page", json!({})).await.unwrap();
    let nested = svc
        .create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "folder".to_string(),
            content: "nested".to_string(),
            parent_id: Some(page),
            position: InsertPositionOwned::End,
            properties: json!({ "colour": "red" }),
            lifecycle_status: None,
        })
        .await
        .unwrap();

    for refused in [
        json!({ "extends": "collection" }),
        json!({ "abstract": true }),
    ] {
        let mut params = json!({
            "schema_id": "folder",
            "rename_fields": [{ "from": "colour", "to": "shade" }],
            "force": true
        });
        for (key, value) in refused.as_object().unwrap() {
            params[key] = value.clone();
        }
        handle_update_schema(&svc, params.clone())
            .await
            .expect_err("the type-system change is refused");

        let schema = svc.get_schema_node("folder").await.unwrap().unwrap();
        assert!(
            schema.get_field("colour").is_some() && schema.get_field("shade").is_none(),
            "{params}: the rename must not have been applied"
        );
        let node = svc.get_node(&nested).await.unwrap().unwrap();
        assert_eq!(node.properties["folder"]["colour"], "red", "{params}");
    }
}
