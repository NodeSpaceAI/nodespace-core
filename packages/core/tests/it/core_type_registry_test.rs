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

/// `bug extends task`, a user-defined subtype of a core type.
async fn seed_bug(svc: &Arc<NodeService>) {
    create_schema(
        svc,
        json!({
            "name": "Bug",
            "extends": "task",
            "fields": [{ "name": "severity", "type": "text" }]
        }),
    )
    .await;
}

#[tokio::test]
async fn a_user_subtype_resolves_to_its_core_type() {
    let (svc, _tmp) = test_service().await;
    seed_bug(&svc).await;

    assert_eq!(
        svc.store().type_chain("bug").await.unwrap(),
        vec!["bug", "task"]
    );
    assert_eq!(
        svc.core_type_of("bug").await.unwrap(),
        Some(CoreNodeType::Task)
    );
    assert!(svc.type_is_a("bug", CoreNodeType::Task).await.unwrap());
    assert!(!svc.type_is_a("bug", CoreNodeType::Project).await.unwrap());
    // The ancestry table agrees with the `extends` edges it is derived from.
    assert_eq!(
        svc.store()
            .get_extends_parent_map()
            .await
            .unwrap()
            .get("bug"),
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
    seed_bug(&svc).await;

    let error = message(create(&svc, "bug", "Bad", json!({ "status": "nonsense" })).await);
    assert!(error.contains("status"), "{error}");

    let id = create(&svc, "bug", "Good", json!({ "severity": "high" }))
        .await
        .unwrap();
    let node = svc.get_node(&id).await.unwrap().unwrap();
    assert_eq!(node.node_type, "bug");
    assert_eq!(node.properties["task"]["status"], "open");
    assert_eq!(node.properties["bug"]["severity"], "high");
}

/// A subtype of `task` is embedded as a task is — not at all — and is titled
/// at any depth, as a task is.
#[tokio::test]
async fn a_subtype_of_task_is_embedded_and_titled_as_a_task() {
    let (svc, _tmp) = test_service().await;
    seed_bug(&svc).await;

    let page = create(&svc, "text", "A page", json!({})).await.unwrap();
    let root_issue = create(&svc, "bug", "A root issue", json!({}))
        .await
        .unwrap();
    let child_issue = create_under(&svc, &page, "bug", "A child issue")
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
        .mention_autocomplete("", Some(100))
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
        error.contains("'colour' is not a field of the type 'task'"),
        "{error}"
    );

    // A primitive declares no fields, so its bucket takes no bare key at all.
    let error = message(create(&svc, "text", "A line", json!({ "colour": "red" })).await);
    assert!(
        error.contains("'colour' is not a field of the type 'text'"),
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

/// A key is accepted when a schema in the chain declares it, in `fields` or in
/// `custom_fields`, or when it starts with `_`; it is a lookup, not a pattern
/// (ADR-112 section 3). A custom field is stored under its bare name in the
/// bucket of the type that declares it.
#[tokio::test]
async fn a_custom_field_is_stored_under_its_bare_name_and_an_undeclared_key_is_refused() {
    let (svc, _tmp) = test_service().await;
    handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "add_fields": [{ "name": "colour", "type": "text" }] }),
    )
    .await
    .expect("a custom field of task");

    let id = create(
        &svc,
        "task",
        "A task",
        json!({ "status": "open", "colour": "red", "_seed": { "tier": "core" } }),
    )
    .await
    .expect("declared and bookkeeping keys are allowed");
    let node = svc.get_node(&id).await.unwrap().unwrap();
    assert_eq!(node.properties["task"]["colour"], "red");
    assert_eq!(node.properties["_seed"]["tier"], "core");

    // Undeclared, and no spelling makes it declared.
    for key in ["shade", "due-date", "team:shade", "vendor:thing", "a b"] {
        let error = message(create(&svc, "task", "A task", json!({ key: "x" })).await);
        // A key of letters, digits, `_` and `-` is echoed; any other is not.
        let shown = if key.contains(':') || key.contains(' ') {
            "a key with other characters"
        } else {
            key
        };
        assert!(
            error.contains(&format!("'{shown}' is not a field of the type 'task'")),
            "{error}"
        );
    }
}

/// A custom field is readable by its bare name through a filter and through
/// a rule (the CEL bridge that plays read).
#[tokio::test]
async fn a_custom_field_is_readable_by_its_bare_name_through_a_filter_and_a_rule() {
    let (svc, _tmp) = test_service().await;
    handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "add_fields": [{ "name": "effort", "type": "number" }] }),
    )
    .await
    .unwrap();
    let small = create(&svc, "task", "Small", json!({ "effort": 1 }))
        .await
        .unwrap();
    create(&svc, "task", "Large", json!({ "effort": 8 }))
        .await
        .unwrap();

    // The agent's and the CLI's query layer: a filter names the field by its
    // bare name and finds the node that holds it.
    let input: nodespace_core::ops::query_ops::ExecuteQueryInput = serde_json::from_value(json!({
        "target_type": "task",
        "filters": [{ "type": "property", "operator": "equals", "property": "effort", "value": 1 }]
    }))
    .unwrap();
    let found = nodespace_core::ops::query_ops::execute_query_nodes(&svc, input)
        .await
        .unwrap();
    assert_eq!(
        found.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
        [small.as_str()]
    );

    // A rule: the CEL bridge exposes the bare name, so a condition on it holds.
    let node = svc.get_node(&small).await.unwrap().unwrap();
    let cel_interpreter::Value::Map(map) = nodespace_core::playbook::cel::node_to_cel_value(&node)
    else {
        panic!("a node is a CEL map");
    };
    let held = map.map.get(&nodespace_core::playbook::cel::key("effort"));
    assert!(
        matches!(
            held,
            Some(cel_interpreter::Value::Int(1)) | Some(cel_interpreter::Value::UInt(1))
        ),
        "node.effort is the stored 1: {held:?}"
    );
}

/// Removing a custom field leaves its values in place (ADR-112, Neutral): the
/// node that holds one is still writable, a write that sets the key afresh is
/// refused, and adding the name again picks the value up.
#[tokio::test]
async fn removing_a_custom_field_leaves_its_values_and_the_node_stays_writable() {
    let (svc, _tmp) = test_service().await;
    handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "add_fields": [{ "name": "effort", "type": "number" }] }),
    )
    .await
    .unwrap();
    let id = create(&svc, "task", "A task", json!({ "effort": 3 }))
        .await
        .unwrap();
    handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "remove_fields": ["effort"], "force": true }),
    )
    .await
    .unwrap();

    let node = svc.get_node(&id).await.unwrap().unwrap();
    assert_eq!(node.properties["task"]["effort"], 3, "the value stays");
    svc.update_node(
        &id,
        node.version,
        NodeUpdate::new().with_properties(json!({ "status": "in_progress" })),
    )
    .await
    .expect("the node stays writable");
    let error = message(create(&svc, "task", "Another", json!({ "effort": 4 })).await);
    assert!(error.contains("'effort' is not a field"), "{error}");

    // Re-adding with a type the stored value does not fit is refused.
    let error = handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "add_fields": [{ "name": "effort", "type": "boolean" }] }),
    )
    .await
    .expect_err("3 is not a boolean")
    .to_string();
    assert!(error.contains("effort"), "{error}");
    handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "add_fields": [{ "name": "effort", "type": "number" }] }),
    )
    .await
    .expect("the same name with a type the value fits");
}

/// The trusted import skips schema validation, not the closed bucket: an
/// undeclared key is refused there as anywhere else, while a declared field and
/// a `_` bookkeeping key are taken.
#[tokio::test]
async fn the_trusted_import_refuses_an_undeclared_key() {
    let (svc, _tmp) = test_service().await;
    let row = |id: &str, node_type: &str, properties: serde_json::Value| {
        (
            id.to_string(),
            node_type.to_string(),
            "A node".to_string(),
            None,
            1.0,
            properties,
        )
    };
    let error = svc
        .bulk_create_hierarchy_trusted(vec![row(
            "00000000-0000-4000-8000-0000000000c1",
            "task",
            json!({ "bogus": 1 }),
        )])
        .await
        .expect_err("an undeclared key on a core type")
        .to_string();
    assert!(
        error.contains("'bogus' is not a field of the type 'task'"),
        "{error}"
    );
    svc.bulk_create_hierarchy_trusted(vec![row(
        "00000000-0000-4000-8000-0000000000c2",
        "task",
        json!({ "priority": "high", "_note": 1 }),
    )])
    .await
    .expect("a declared field and a bookkeeping key");
}

/// A custom field cannot be named after what an instance reads as a
/// relationship under its reverse name (a task's `blocked_by`, read from the
/// `blocks` of another task, and its `project`, read from a project's `tasks`),
/// and a relationship cannot be given a reverse name that a type at its target
/// end declares as a field (ADR-112 section 5, both directions).
#[tokio::test]
async fn reverse_relationship_names_are_name_clashes_in_both_directions() {
    let (svc, _tmp) = test_service().await;
    for name in ["blocked_by", "blocks"] {
        let error = handle_update_schema(
            &svc,
            json!({ "schema_id": "task", "add_fields": [{ "name": name, "type": "text" }] }),
        )
        .await
        .expect_err("a relationship name read from a task")
        .to_string();
        assert!(
            error.contains(name) && error.contains("relationship"),
            "{error}"
        );
    }
    // A type extending task reads the same names.
    handle_create_schema(
        &svc,
        json!({ "name": "Chore", "extends": "task", "fields": [] }),
    )
    .await
    .unwrap();
    let error = handle_update_schema(
        &svc,
        json!({ "schema_id": "chore", "add_fields": [{ "name": "blocked_by", "type": "text" }] }),
    )
    .await
    .expect_err("an inherited reverse name")
    .to_string();
    assert!(error.contains("blocked_by"), "{error}");

    // A reverse name a user type declares toward task is read from a task too.
    create_schema(&svc, json!({ "name": "Crew", "fields": [] })).await;
    handle_update_schema(
        &svc,
        json!({
            "schema_id": "crew",
            "add_relationships": [{
                "name": "assigned",
                "targetType": "task",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "crew_of",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .unwrap();
    let error = handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "add_fields": [{ "name": "crew_of", "type": "text" }] }),
    )
    .await
    .expect_err("a user type's reverse name")
    .to_string();
    assert!(
        error.contains("crew_of") && error.contains("relationship"),
        "{error}"
    );

    // The other direction: a field on task, then a relationship whose reverse
    // name is that field.
    handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "add_fields": [{ "name": "owner_note", "type": "text" }] }),
    )
    .await
    .unwrap();
    create_schema(&svc, json!({ "name": "Team", "fields": [] })).await;
    let error = handle_update_schema(
        &svc,
        json!({
            "schema_id": "team",
            "add_relationships": [{
                "name": "work",
                "targetType": "task",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "owner_note",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .expect_err("a reverse name that is a field of the target")
    .to_string();
    assert!(
        error.contains("owner_note") && error.contains("reverse name"),
        "{error}"
    );
}

/// `add_field_values` extends a `core` field's list but not a `system` field's:
/// the maintaining code owns those values.
#[tokio::test]
async fn values_cannot_be_added_to_a_system_field() {
    let (svc, _tmp) = test_service().await;
    let system = nodespace_core::models::SchemaField {
        name: "stage".to_string(),
        friendly_name: "Stage".to_string(),
        field_type: nodespace_core::models::SchemaFieldType::Enum,
        protection: nodespace_core::models::SchemaProtectionLevel::System,
        extensible: Some(true),
        core_values: Some(vec![nodespace_types::EnumValue::new(
            "open".to_string(),
            "Open".to_string(),
        )]),
        ..Default::default()
    };
    nodespace_core::schema::declare_system_custom_fields(&svc, "person", vec![system])
        .await
        .expect("an extension declares a system field");
    let error = handle_update_schema(
        &svc,
        json!({
            "schema_id": "person",
            "add_field_values": [{ "field": "stage", "values": [{ "value": "late", "label": "Late" }] }]
        }),
    )
    .await
    .expect_err("a system field's values are not the caller's")
    .to_string();
    assert!(
        error.contains("stage") && error.contains("system"),
        "{error}"
    );
    // A core field's extensible list still takes values.
    handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "add_field_values": [{ "field": "status", "values": [{ "value": "parked", "label": "Parked" }] }]
        }),
    )
    .await
    .expect("a core field's list is the user's to extend");
}

/// A custom field cannot take an envelope key or a type's name: either would be
/// read as that, not as the field.
#[tokio::test]
async fn reserved_names_cannot_be_custom_fields() {
    let (svc, _tmp) = test_service().await;
    for name in [
        "id",
        "content",
        "title",
        "version",
        "node_type",
        "task",
        "issue",
    ] {
        let error = handle_update_schema(
            &svc,
            json!({ "schema_id": "task", "add_fields": [{ "name": name, "type": "text" }] }),
        )
        .await
        .expect_err("a reserved name")
        .to_string();
        assert!(error.contains(&format!("'{name}'")), "{name}: {error}");
    }
    handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "add_fields": [{ "name": "effort", "type": "text" }] }),
    )
    .await
    .unwrap();
    let error = handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "rename_fields": [{ "from": "effort", "to": "title" }],
            "force": true
        }),
    )
    .await
    .expect_err("a rename to a reserved name")
    .to_string();
    assert!(error.contains("'title'"), "{error}");
}

/// The agent can mint no protected field: `create_schema` takes user fields only.
#[tokio::test]
async fn create_schema_refuses_a_system_or_core_field() {
    let (svc, _tmp) = test_service().await;
    for protection in ["system", "core"] {
        let error = handle_create_schema(
            &svc,
            json!({
                "name": "Gadget",
                "fields": [{ "name": "stamp", "type": "text", "protection": protection }]
            }),
        )
        .await
        .expect_err("a protected field on a user-defined type")
        .to_string();
        assert!(
            error.contains("stamp") && error.contains(protection),
            "{error}"
        );
    }
    assert!(svc.get_schema_node("gadget").await.unwrap().is_none());
}

/// An extension declares system custom fields on a built-in type only; a
/// user-defined or missing schema is refused, declaring again is a no-op, and a
/// later declaration of the same name and type with other metadata is taken as
/// the same field rather than refused.
#[tokio::test]
async fn declaring_system_custom_fields_is_for_built_in_types() {
    use nodespace_core::models::{SchemaField, SchemaFieldType, SchemaProtectionLevel};
    use nodespace_core::schema::declare_system_custom_fields;
    let (svc, _tmp) = test_service().await;
    let field = |description: &str| SchemaField {
        name: "stamp".to_string(),
        friendly_name: "Stamp".to_string(),
        field_type: SchemaFieldType::Text,
        protection: SchemaProtectionLevel::System,
        description: Some(description.to_string()),
        ..Default::default()
    };
    create_schema(&svc, json!({ "name": "Gadget", "fields": [] })).await;
    let error = declare_system_custom_fields(&svc, "gadget", vec![field("a")])
        .await
        .expect_err("a user-defined type has no custom_fields")
        .to_string();
    assert!(error.contains("user-defined"), "{error}");
    let error = declare_system_custom_fields(&svc, "no-such-type", vec![field("a")])
        .await
        .expect_err("no such schema")
        .to_string();
    assert!(error.contains("not found"), "{error}");

    declare_system_custom_fields(&svc, "person", vec![field("a")])
        .await
        .unwrap();
    declare_system_custom_fields(&svc, "person", vec![field("changed later")])
        .await
        .expect("the same name and type is the field already declared");
    let stored = svc.get_schema_node("person").await.unwrap().unwrap();
    assert_eq!(stored.custom_fields.len(), 1);
    assert_eq!(stored.custom_fields[0].description.as_deref(), Some("a"));
}

/// The carried-back exception covers the whole chain: a leftover value held in
/// an ancestor's bucket is still carried back when a flat update on a subtype
/// echoes it, though flat normalization files the key under the node's own.
#[tokio::test]
async fn a_leftover_in_an_ancestors_bucket_is_carried_back_by_a_flat_update() {
    let (svc, _tmp) = test_service().await;
    handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "add_fields": [{ "name": "effort", "type": "number" }] }),
    )
    .await
    .unwrap();
    create_schema(
        &svc,
        json!({ "name": "Chore", "extends": "task", "fields": [] }),
    )
    .await;
    let id = create(&svc, "chore", "A chore", json!({ "effort": 3 }))
        .await
        .unwrap();
    let node = svc.get_node(&id).await.unwrap().unwrap();
    assert_eq!(
        node.properties["task"]["effort"], 3,
        "held in the ancestor's bucket"
    );
    handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "remove_fields": ["effort"], "force": true }),
    )
    .await
    .unwrap();

    svc.update_node(
        &id,
        node.version,
        NodeUpdate::new().with_properties(json!({ "effort": 3, "status": "in_progress" })),
    )
    .await
    .expect("the echoed leftover is not a write of it");
    let error = message(
        svc.update_node(
            &id,
            node.version + 1,
            NodeUpdate::new().with_properties(json!({ "effort": 4 })),
        )
        .await,
    );
    assert!(error.contains("'effort' is not a field"), "{error}");
}

/// `NodeService::bulk_create` takes flat properties; they are filed under the
/// node's type first, so the closed bucket and the system field check hold
/// there as on every other write.
#[tokio::test]
async fn bulk_create_holds_the_closed_bucket_and_the_system_field_check() {
    let (svc, _tmp) = test_service().await;
    let error = svc
        .bulk_create(vec![Node::new(
            "task".to_string(),
            "A task".to_string(),
            json!({ "bogus": 1 }),
        )])
        .await
        .expect_err("an undeclared key")
        .to_string();
    assert!(
        error.contains("'bogus' is not a field of the type 'task'"),
        "{error}"
    );

    let error = svc
        .bulk_create(vec![Node::new(
            "ai-chat-pty".to_string(),
            "A chat".to_string(),
            json!({ "agent": "claude-code", "summary": "forged" }),
        )])
        .await
        .expect_err("a system field")
        .to_string();
    assert!(
        error.contains("summary") && error.contains("system"),
        "{error}"
    );

    svc.bulk_create(vec![Node::new(
        "task".to_string(),
        "A task".to_string(),
        json!({ "priority": "high" }),
    )])
    .await
    .expect("a declared field");
}

/// The relationship-name check judges the schema as the call leaves it: a
/// rename cannot take a name the chain reads as a relationship by passing an
/// empty `remove_relationships` list or by naming the parent it already has;
/// moving under a parent that reads a name is refused for the type's own
/// fields too; and a name the call frees by removing its relationship is
/// still free.
#[tokio::test]
async fn relationship_names_are_judged_against_the_chain_the_call_leaves() {
    let (svc, _tmp) = test_service().await;
    handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "add_fields": [{ "name": "effort", "type": "number" }] }),
    )
    .await
    .unwrap();
    let error = handle_update_schema(
        &svc,
        json!({
            "schema_id": "task",
            "rename_fields": [{ "from": "effort", "to": "blocked_by" }],
            "remove_relationships": [],
            "force": true
        }),
    )
    .await
    .expect_err("an empty removal list frees nothing")
    .to_string();
    assert!(
        error.contains("blocked_by") && error.contains("relationship"),
        "{error}"
    );

    create_schema(
        &svc,
        json!({ "name": "Chore", "extends": "task", "fields": [{ "name": "room", "type": "text" }] }),
    )
    .await;
    let error = handle_update_schema(
        &svc,
        json!({
            "schema_id": "chore",
            "rename_fields": [{ "from": "room", "to": "blocked_by" }],
            "extends": "task",
            "force": true
        }),
    )
    .await
    .expect_err("the parent it already has still reads the name")
    .to_string();
    assert!(error.contains("blocked_by"), "{error}");

    // F2: a field the type already holds, moved under a parent that reads it.
    create_schema(
        &svc,
        json!({ "name": "Foo", "fields": [{ "name": "blocked_by", "type": "text" }] }),
    )
    .await;
    let error = handle_update_schema(&svc, json!({ "schema_id": "foo", "extends": "task" }))
        .await
        .expect_err("task reads blocked_by as a relationship")
        .to_string();
    assert!(error.contains("blocked_by"), "{error}");
    assert!(svc
        .get_schema_node("foo")
        .await
        .unwrap()
        .unwrap()
        .extends
        .is_none());
}

/// A reverse name is a clash with a field of the schema being created, and a
/// wildcard relationship's reverse name with a field of any type.
#[tokio::test]
async fn a_reverse_name_clashes_with_a_field_of_its_own_schema_or_any_type() {
    let (svc, _tmp) = test_service().await;
    let error = handle_create_schema(
        &svc,
        json!({
            "name": "Folk2",
            "fields": [{ "name": "kin_of", "type": "text" }],
            "relationships": [{
                "name": "kin",
                "targetType": "folk2",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "kin_of",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .expect_err("the reverse name is its own field")
    .to_string();
    assert!(
        error.contains("kin_of") && error.contains("reverse name"),
        "{error}"
    );
    assert!(svc.get_schema_node("folk2").await.unwrap().is_none());

    handle_update_schema(
        &svc,
        json!({ "schema_id": "task", "add_fields": [{ "name": "effort", "type": "number" }] }),
    )
    .await
    .unwrap();
    create_schema(&svc, json!({ "name": "Anchor", "fields": [] })).await;
    let error = handle_update_schema(
        &svc,
        json!({
            "schema_id": "anchor",
            "add_relationships": [{
                "name": "pins",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "effort",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .expect_err("a wildcard reverse name read from a task")
    .to_string();
    assert!(
        error.contains("effort") && error.contains("reverse name"),
        "{error}"
    );
}

/// Names the CEL bridge overwrites or the schema layer reserves cannot be
/// custom fields: `type_chain`, `parent_id`, `root_id` and the others.
#[tokio::test]
async fn the_bridges_own_keys_cannot_be_custom_fields() {
    let (svc, _tmp) = test_service().await;
    for name in [
        "type_chain",
        "parent_id",
        "root_id",
        "created_at",
        "modified_at",
    ] {
        let error = handle_update_schema(
            &svc,
            json!({ "schema_id": "task", "add_fields": [{ "name": name, "type": "text" }] }),
        )
        .await
        .expect_err("a reserved name")
        .to_string();
        assert!(error.contains(&format!("'{name}'")), "{name}: {error}");
    }
}

/// No schema is open (ADR-112 section 11): a user subtype of a core type and a
/// user-defined type refuse a key no schema in their chain declares, naming
/// the key and the type.
#[tokio::test]
async fn a_subtype_and_a_user_defined_type_are_closed() {
    let (svc, _tmp) = test_service().await;
    seed_bug(&svc).await;
    create_schema(
        &svc,
        json!({ "name": "Invoice", "fields": [{ "name": "total", "type": "number" }] }),
    )
    .await;

    let error = message(create(&svc, "bug", "An issue", json!({ "anything": 1 })).await);
    assert!(
        error.contains("'anything' is not a field of the type 'bug'"),
        "{error}"
    );
    let error = message(create(&svc, "invoice", "An invoice", json!({ "anything": 1 })).await);
    assert!(
        error.contains("'anything' is not a field of the type 'invoice'"),
        "{error}"
    );
    create(&svc, "invoice", "An invoice", json!({ "total": 1 }))
        .await
        .expect("a declared field");
}

/// `undeclared_keys` says ahead of a write which flat keys the closed
/// bucket will refuse. The two must agree key for key, in both directions: a
/// key it lists that is accepted would be renamed by a caller for nothing,
/// and one it misses is still refused.
#[tokio::test]
async fn undeclared_keys_lists_exactly_the_keys_a_write_refuses() {
    let (svc, _tmp) = test_service().await;
    handle_create_schema(
        &svc,
        json!({ "name": "Gadget", "fields": [{ "name": "size", "type": "number" }] }),
    )
    .await
    .expect("a user-defined type");
    handle_create_schema(
        &svc,
        json!({
            "name": "Chore",
            "extends": "task",
            "fields": [{ "name": "room", "type": "text" }]
        }),
    )
    .await
    .expect("a user subtype of a core type");

    let object = json!({ "status": "open" });
    let text = json!("x");
    let cases = [
        ("colour", &text),
        ("task__status", &text),
        ("status", &text),
        ("priority", &text),
        ("room", &text),
        ("size", &text),
        ("vendor:thing", &text),
        ("_seed", &text),
        // A bucket's own name addresses the bucket when it holds an object,
        // and is one more undeclared key when it holds anything else.
        ("task", &object),
        ("task", &text),
        // Derived from a checkbox's content, with a refusal of its own.
        ("checked", &text),
    ];
    // `schema` among them: its definition is not bucketed, so the store
    // refuses none of its keys this way.
    for node_type in ["task", "text", "checkbox", "gadget", "chore", "schema"] {
        for (key, value) in cases {
            let properties = json!({ key: value });
            let listed = svc
                .undeclared_keys(node_type, properties.as_object().unwrap())
                .await
                .unwrap();
            // Refused for this reason, by this message. A write refused for
            // another (a value `status` does not take, a derived attribute)
            // is not one the closed bucket turned away.
            let refused = create(&svc, node_type, "One", properties.clone())
                .await
                .is_err_and(|e| e.to_string().contains("is not a field of the type"));
            assert_eq!(
                listed.iter().any(|k| k == key),
                refused,
                "{key:?}: {value} on a {node_type}"
            );
        }
    }
    // A flat key beside the node's own bucket: the map is taken as bucketed
    // already, so the key reaches no bucket and is not refused this way.
    let beside = json!({ "task": { "status": "open" }, "colour": "red" });
    let listed = svc
        .undeclared_keys("task", beside.as_object().unwrap())
        .await
        .unwrap();
    let refused = create(&svc, "task", "One", beside.clone())
        .await
        .is_err_and(|e| e.to_string().contains("is not a field of the type"));
    assert_eq!(!listed.is_empty(), refused, "a key beside the own bucket");

    // A type that does not exist lists nothing: that is the write's own error.
    let colour = json!({ "colour": "red" });
    assert!(svc
        .undeclared_keys("no-such-type", colour.as_object().unwrap())
        .await
        .unwrap()
        .is_empty());
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
    seed_bug(&svc).await;
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
    svc.ensure_no_typed_core_fields(&task, None, &json!({ "colour": "red" }))
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
    let issue = create(&svc, "bug", "An issue", json!({})).await.unwrap();
    svc.ensure_no_typed_core_fields(&issue, None, &json!({ "status": "done" }))
        .await
        .expect("a subtype is written through the generic update");

    // The same holds for every core type with a typed update.
    let collection = create(&svc, "collection", "notes", json!({}))
        .await
        .unwrap();
    let skill = create(
        &svc,
        "skill",
        "Research",
        json!({ "use_for": "Search", "tool_whitelist": [] }),
    )
    .await
    .unwrap();
    for (node, patch, refusal) in [
        (
            collection.as_str(),
            json!({ "description": "My notes" }),
            "typed collection update",
        ),
        (
            skill.as_str(),
            json!({ "maxIterations": 3 }),
            "typed skill update",
        ),
        (
            "database-settings-singleton",
            json!({ "required_extensions": [] }),
            "typed database-settings update",
        ),
    ] {
        let error = message(svc.ensure_no_typed_core_fields(node, None, &patch).await);
        assert!(error.contains(refusal), "{patch}: {error}");
    }

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

/// An edge field's type is the same vocabulary, refused the same way.
#[tokio::test]
async fn string_is_not_an_edge_field_type() {
    let (svc, _tmp) = test_service().await;
    create_schema(&svc, json!({ "name": "Team", "fields": [] })).await;

    let declare = |edge_type: &str| {
        json!({
            "name": "Engineer",
            "fields": [],
            "relationships": [{
                "name": "works_on",
                "targetType": "team",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "engineers",
                "reverseCardinality": "many",
                "edgeFields": [{ "name": "note", "type": edge_type }]
            }]
        })
    };

    let error = handle_create_schema(&svc, declare("string"))
        .await
        .expect_err("'string' is not an edge field type")
        .to_string();
    assert!(error.contains("use 'text'"), "{error}");

    let error = handle_create_schema(&svc, declare("record"))
        .await
        .expect_err("an unknown edge field type must be refused")
        .to_string();
    assert!(error.contains("field type"), "{error}");

    create_schema(&svc, declare("text")).await;
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
        .get_schema_node("customer-profile")
        .await
        .unwrap()
        .is_some());

    // The type-name form is the schema's alone.
    let error = message(svc.create_node(with_id("customer-profile", "text")).await);
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
                "targetType": "support-chat",
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
        "support-chat",
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
