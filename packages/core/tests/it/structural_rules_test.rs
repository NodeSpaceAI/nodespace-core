//! The structural rules, end to end against a real store (ADR-089): each
//! type declares which children its nodes may have and where they may sit,
//! a subtype only tightens what it inherits, and every path that writes a
//! `has_child` edge, makes a node a root, or retypes a node is held to both
//! rules.

use nodespace_core::db::{SqliteStore, TreeInvariantRule};
use nodespace_core::models::{
    CoreNodeType, Node, NodeUpdate, SchemaChildrenRule, SchemaParentRule,
};
use nodespace_core::schema::{handle_create_schema, handle_update_schema};
use nodespace_core::services::{
    CreateNodeParams, InsertPosition, InsertPositionOwned, NodeService, NodeServiceError,
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

async fn create(svc: &Arc<NodeService>, node_type: &str, content: &str) -> String {
    try_create(svc, node_type, content)
        .await
        .unwrap_or_else(|e| panic!("creating a {node_type} failed: {e:?}"))
}

async fn try_create(
    svc: &Arc<NodeService>,
    node_type: &str,
    content: &str,
) -> Result<String, NodeServiceError> {
    svc.create_node(Node::new(
        node_type.to_string(),
        content.to_string(),
        json!({}),
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

async fn move_under(
    svc: &Arc<NodeService>,
    node_id: &str,
    parent_id: Option<&str>,
) -> Result<(), NodeServiceError> {
    let version = svc.get_node(node_id).await.unwrap().unwrap().version;
    svc.move_node(node_id, version, parent_id, InsertPosition::End)
        .await
        .map(|_| ())
}

async fn retype(
    svc: &Arc<NodeService>,
    node_id: &str,
    node_type: &str,
) -> Result<(), NodeServiceError> {
    let version = svc.get_node(node_id).await.unwrap().unwrap().version;
    svc.update_node(
        node_id,
        version,
        NodeUpdate::new().with_node_type(node_type.to_string()),
    )
    .await
    .map(|_| ())
}

/// The rule a refused write names.
fn refused<T: std::fmt::Debug>(result: Result<T, NodeServiceError>) -> TreeInvariantRule {
    match result.expect_err("the write must be refused") {
        NodeServiceError::TreeInvariantViolation(v) => v.rule,
        other => panic!("expected a tree invariant violation, got {other:?}"),
    }
}

async fn parent_of(svc: &Arc<NodeService>, node_id: &str) -> Option<String> {
    svc.get_parent(node_id).await.unwrap().map(|p| p.id)
}

/// `thread`, `reply` (only under a thread, no children) and `journal` (no
/// task children): the three rules no core type declares today.
async fn seed_user_rules(svc: &Arc<NodeService>) {
    create_schema(svc, json!({ "name": "Thread", "fields": [] })).await;
    create_schema(
        svc,
        json!({
            "name": "Reply",
            "fields": [],
            "children": { "rule": "none" },
            "parent": { "rule": "must_have_parent_of", "types": ["thread"] }
        }),
    )
    .await;
    create_schema(
        svc,
        json!({
            "name": "Journal",
            "fields": [],
            "children": { "rule": "any_except", "types": ["task"] }
        }),
    )
    .await;
}

// ---------------------------------------------------------------------------
// Declaration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_schema_declares_its_rules_and_the_store_resolves_them() {
    let (svc, _tmp) = test_service().await;
    seed_user_rules(&svc).await;

    let reply = svc.get_schema_node("reply").await.unwrap().unwrap();
    assert_eq!(reply.children, SchemaChildrenRule::None);
    assert_eq!(
        reply.parent,
        SchemaParentRule::MustHaveParentOf {
            types: vec!["thread".to_string()]
        }
    );
    // `any` is the default, and is not stored.
    let thread = svc.get_schema_node("thread").await.unwrap().unwrap();
    assert!(thread.children.is_any() && thread.parent.is_any());
    let stored = svc.store().get_node("thread").await.unwrap().unwrap();
    assert!(stored.properties.get("children").is_none());
    assert!(stored.properties.get("parent").is_none());

    assert_eq!(
        svc.store()
            .structural_rules_in_force("reply")
            .await
            .unwrap(),
        (reply.children, reply.parent)
    );
    assert_eq!(
        svc.store()
            .structural_rules_in_force("journal")
            .await
            .unwrap(),
        (
            SchemaChildrenRule::AnyExcept {
                types: vec!["task".to_string()]
            },
            SchemaParentRule::Any
        )
    );
}

/// The store resolves every core type to the rules the registry holds, and a
/// seeded core schema declares them.
#[tokio::test]
async fn the_store_and_the_seeded_schemas_carry_the_registrys_rules() {
    let (svc, _tmp) = test_service().await;
    for core in CoreNodeType::ALL {
        let in_force = core.structure();
        assert_eq!(
            svc.store()
                .structural_rules_in_force(core.as_str())
                .await
                .unwrap(),
            (in_force.children.into(), in_force.parent.into()),
            "{core}"
        );
        if let Some(schema) = svc.get_schema_node(core.as_str()).await.unwrap() {
            let declared = core.declared_structure();
            assert_eq!(schema.children, declared.children.into(), "{core}");
            assert_eq!(schema.parent, declared.parent.into(), "{core}");
        }
    }
}

#[tokio::test]
async fn a_rule_must_name_types_that_exist() {
    let (svc, _tmp) = test_service().await;
    let error = handle_create_schema(
        &svc,
        json!({
            "name": "Reply",
            "fields": [],
            "parent": { "rule": "must_have_parent_of", "types": ["thread"] }
        }),
    )
    .await
    .expect_err("no `thread` type exists")
    .to_string();
    assert!(error.contains("thread"), "{error}");
    assert!(svc.get_schema_node("reply").await.unwrap().is_none());

    let error = handle_create_schema(
        &svc,
        json!({ "name": "Reply", "fields": [], "children": { "rule": "any_except", "types": [] } }),
    )
    .await
    .expect_err("an empty list names nothing")
    .to_string();
    assert!(error.contains("names no types"), "{error}");

    // A rule may name the type that declares it.
    create_schema(
        &svc,
        json!({
            "name": "Section",
            "fields": [],
            "children": { "rule": "any_except", "types": ["section"] }
        }),
    )
    .await;
}

// ---------------------------------------------------------------------------
// A subtype only tightens
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_subtype_inherits_its_bases_rules_and_cannot_relax_them() {
    let (svc, _tmp) = test_service().await;
    seed_user_rules(&svc).await;
    create_schema(&svc, json!({ "name": "Topic", "fields": [] })).await;
    create_schema(
        &svc,
        json!({ "name": "Support Thread", "extends": "thread", "fields": [] }),
    )
    .await;

    // Inherited as they are.
    create_schema(
        &svc,
        json!({ "name": "Quick Reply", "extends": "reply", "fields": [] }),
    )
    .await;
    assert_eq!(
        svc.store()
            .structural_rules_in_force("quick_reply")
            .await
            .unwrap(),
        svc.store()
            .structural_rules_in_force("reply")
            .await
            .unwrap()
    );

    // Relaxing `children: none`, or widening the parent list, is refused.
    for relaxed in [
        json!({ "children": { "rule": "any_except", "types": ["task"] } }),
        json!({ "parent": { "rule": "must_have_parent_of", "types": ["topic"] } }),
        json!({ "parent": { "rule": "must_have_parent_of", "types": ["thread", "topic"] } }),
        json!({ "parent": { "rule": "must_be_root" } }),
    ] {
        let mut params = json!({ "name": "Loose Reply", "extends": "reply", "fields": [] });
        params
            .as_object_mut()
            .unwrap()
            .extend(relaxed.as_object().unwrap().clone());
        let error = handle_create_schema(&svc, params.clone())
            .await
            .expect_err("a subtype may only tighten")
            .to_string();
        assert!(error.contains("only tighten"), "{params}: {error}");
        assert!(svc.get_schema_node("loose_reply").await.unwrap().is_none());
    }
    let error = handle_create_schema(
        &svc,
        json!({
            "name": "Nested Collection",
            "extends": "collection",
            "fields": [],
            "parent": { "rule": "must_have_parent_of", "types": ["thread"] }
        }),
    )
    .await
    .expect_err("a collection is a root, whatever its subtype says")
    .to_string();
    assert!(error.contains("only tighten"), "{error}");

    // Narrowing the list to a subtype of a named type tightens.
    create_schema(
        &svc,
        json!({
            "name": "Support Reply",
            "extends": "reply",
            "fields": [],
            "parent": { "rule": "must_have_parent_of", "types": ["support_thread"] }
        }),
    )
    .await;
    let thread = create(&svc, "thread", "General").await;
    let support = create(&svc, "support_thread", "Support").await;
    assert_eq!(
        refused(create_under(&svc, &thread, "support_reply", "hello").await),
        TreeInvariantRule::ParentRequired
    );
    create_under(&svc, &support, "support_reply", "hello")
        .await
        .unwrap();

    // An `any_except` list adds to the base's.
    create_schema(
        &svc,
        json!({
            "name": "Private Journal",
            "extends": "journal",
            "fields": [],
            "children": { "rule": "any_except", "types": ["person"] }
        }),
    )
    .await;
    let journal = create(&svc, "private_journal", "Mine").await;
    for refused_type in ["task", "person"] {
        assert_eq!(
            refused(create_under(&svc, &journal, refused_type, "x").await),
            TreeInvariantRule::ChildNotAllowed,
            "{refused_type}"
        );
    }
    create_under(&svc, &journal, "text", "a line")
        .await
        .unwrap();
}

/// Loosening is always safe for existing data, so a rule can go back to
/// `any`, and a rule that changes kind keeps nothing of the old one.
#[tokio::test]
async fn a_rule_can_be_loosened_back_to_any() {
    let (svc, _tmp) = test_service().await;
    create_schema(
        &svc,
        json!({
            "name": "Folder",
            "fields": [],
            "children": { "rule": "any_except", "types": ["task"] },
            "parent": { "rule": "must_be_root" }
        }),
    )
    .await;
    let page = create(&svc, "text", "A page").await;
    let folder = create(&svc, "folder", "top").await;
    assert_eq!(
        refused(move_under(&svc, &folder, Some(&page)).await),
        TreeInvariantRule::MustBeRoot
    );

    // One rule changes kind: nothing of the old list is kept.
    handle_update_schema(
        &svc,
        json!({ "schema_id": "folder", "children": { "rule": "none" } }),
    )
    .await
    .unwrap();
    let stored = svc.store().get_node("folder").await.unwrap().unwrap();
    assert_eq!(stored.properties["children"], json!({ "rule": "none" }));
    assert_eq!(
        refused(create_under(&svc, &folder, "text", "inside").await),
        TreeInvariantRule::ChildrenNone
    );

    // Both go back to `any`: the keys leave the schema, and the writes that
    // were refused go through.
    handle_update_schema(
        &svc,
        json!({
            "schema_id": "folder",
            "children": { "rule": "any" },
            "parent": { "rule": "any" }
        }),
    )
    .await
    .unwrap();
    let stored = svc.store().get_node("folder").await.unwrap().unwrap();
    assert!(stored.properties.get("children").is_none(), "{stored:?}");
    assert!(stored.properties.get("parent").is_none(), "{stored:?}");
    assert_eq!(
        svc.store()
            .structural_rules_in_force("folder")
            .await
            .unwrap(),
        (SchemaChildrenRule::Any, SchemaParentRule::Any)
    );
    create_under(&svc, &folder, "task", "a task inside")
        .await
        .unwrap();
    move_under(&svc, &folder, Some(&page)).await.unwrap();

    // Tightening again is checked against the nodes the type now has.
    handle_update_schema(
        &svc,
        json!({ "schema_id": "folder", "parent": { "rule": "any" }, "children": { "rule": "none" } }),
    )
    .await
    .expect_err("the folder has a child now");

    // An update that names neither rule leaves both as they are.
    create_schema(
        &svc,
        json!({ "name": "Box", "fields": [], "children": { "rule": "none" } }),
    )
    .await;
    handle_update_schema(
        &svc,
        json!({ "schema_id": "box", "description": "A closed box" }),
    )
    .await
    .unwrap();
    assert_eq!(
        svc.store()
            .structural_rules_in_force("box")
            .await
            .unwrap()
            .0,
        SchemaChildrenRule::None
    );
}

/// The same write clears the `abstract` flag: a type made concrete again can
/// be instantiated.
#[tokio::test]
async fn an_abstract_type_can_be_made_concrete_again() {
    let (svc, _tmp) = test_service().await;
    create_schema(
        &svc,
        json!({ "name": "Shape", "fields": [], "abstract": true }),
    )
    .await;
    assert!(try_create(&svc, "shape", "a shape").await.is_err());

    handle_update_schema(&svc, json!({ "schema_id": "shape", "abstract": false }))
        .await
        .unwrap();
    assert!(!svc.store().is_abstract_type("shape").await.unwrap());
    try_create(&svc, "shape", "a shape").await.unwrap();
}

#[tokio::test]
async fn a_core_types_rules_cannot_be_changed() {
    let (svc, _tmp) = test_service().await;
    for change in [
        json!({ "schema_id": "query", "children": { "rule": "any" } }),
        json!({ "schema_id": "collection", "parent": { "rule": "any" } }),
        json!({ "schema_id": "text", "children": { "rule": "none" } }),
    ] {
        let error = handle_update_schema(&svc, change.clone())
            .await
            .expect_err("a core type's rules are the registry's")
            .to_string();
        assert!(error.contains("core type"), "{change}: {error}");
    }
    assert_eq!(
        svc.store()
            .structural_rules_in_force("query")
            .await
            .unwrap()
            .0,
        SchemaChildrenRule::None
    );
}

/// Tightening a rule on an existing type is refused while a node of the type
/// sits where the new rule would not have admitted it, and applies otherwise.
#[tokio::test]
async fn a_rule_is_tightened_only_when_no_node_breaks_it() {
    let (svc, _tmp) = test_service().await;
    create_schema(&svc, json!({ "name": "Folder", "fields": [] })).await;
    create_schema(&svc, json!({ "name": "Card", "fields": [] })).await;
    let page = create(&svc, "text", "A page").await;
    let folder = create(&svc, "folder", "top").await;
    let nested = create_under(&svc, &page, "folder", "nested").await.unwrap();
    create_under(&svc, &folder, "text", "inside").await.unwrap();

    for (change, breaks) in [
        (
            json!({ "parent": { "rule": "must_be_root" } }),
            "has a parent",
        ),
        (json!({ "children": { "rule": "none" } }), "has children"),
        (
            json!({ "children": { "rule": "any_except", "types": ["text"] } }),
            "has a child",
        ),
        (
            json!({ "parent": { "rule": "must_have_parent_of", "types": ["card"] } }),
            "has no parent",
        ),
    ] {
        let mut params = json!({ "schema_id": "folder" });
        params
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        let error = handle_update_schema(&svc, params.clone())
            .await
            .expect_err("an existing folder breaks the rule")
            .to_string();
        assert!(error.contains(breaks), "{params}: {error}");
    }
    assert_eq!(
        svc.store()
            .structural_rules_in_force("folder")
            .await
            .unwrap(),
        (SchemaChildrenRule::Any, SchemaParentRule::Any)
    );

    // With the offender out of the way the rule applies, and binds from then on.
    move_under(&svc, &nested, None).await.unwrap();
    handle_update_schema(
        &svc,
        json!({ "schema_id": "folder", "parent": { "rule": "must_be_root" } }),
    )
    .await
    .unwrap();
    assert_eq!(
        refused(move_under(&svc, &nested, Some(&page)).await),
        TreeInvariantRule::MustBeRoot
    );

    // A type extending this one must not be left relaxing it.
    create_schema(&svc, json!({ "name": "Deck", "fields": [] })).await;
    create_schema(
        &svc,
        json!({
            "name": "Card Stack",
            "extends": "deck",
            "fields": [],
            "children": { "rule": "any_except", "types": ["task"] }
        }),
    )
    .await;
    let error = handle_update_schema(
        &svc,
        json!({ "schema_id": "deck", "children": { "rule": "none" } }),
    )
    .await
    .expect_err("card_stack's list would relax `none`")
    .to_string();
    assert!(error.contains("card_stack"), "{error}");
}

// ---------------------------------------------------------------------------
// Root-only types
// ---------------------------------------------------------------------------

#[tokio::test]
async fn collection_schema_and_date_are_roots_and_so_are_their_subtypes() {
    let (svc, _tmp) = test_service().await;
    create_schema(
        &svc,
        json!({ "name": "Team", "extends": "collection", "fields": [] }),
    )
    .await;
    let page = create(&svc, "text", "A page").await;

    // Created under a parent.
    for root_only in ["collection", "team", "schema"] {
        assert_eq!(
            refused(create_under(&svc, &page, root_only, "Nested").await),
            TreeInvariantRule::MustBeRoot,
            "{root_only}"
        );
    }

    // Moved under a parent.
    let collection = create(&svc, "collection", "Work").await;
    let team = create(&svc, "team", "Core team").await;
    let date = svc
        .create_node(Node::new_with_id(
            "2026-03-04".to_string(),
            "date".to_string(),
            "2026-03-04".to_string(),
            json!({}),
        ))
        .await
        .unwrap();
    for root in [&collection, &team, &date] {
        assert_eq!(
            refused(move_under(&svc, root, Some(&page)).await),
            TreeInvariantRule::MustBeRoot,
            "{root}"
        );
        assert_eq!(parent_of(&svc, root).await, None);
    }

    // Each still takes children of its own.
    for root in [&collection, &team, &date] {
        create_under(&svc, root, "text", "a line").await.unwrap();
    }
}

// ---------------------------------------------------------------------------
// Leaf types
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_leaf_types_refuse_children_on_every_path() {
    let (svc, _tmp) = test_service().await;
    let note = create(&svc, "text", "a note").await;
    let tool = svc
        .create_node(Node::new(
            "tool".to_string(),
            "lookup".to_string(),
            json!({ "description": "Looks a thing up",
                "handler": "lookup", "parameter_schema": { "type": "object" } }),
        ))
        .await
        .expect("a tool node");
    let settings = create(&svc, "database-settings", "Settings").await;
    let mut leaves: Vec<(&str, String)> = vec![("tool", tool), ("database-settings", settings)];
    for (leaf_type, content) in [
        ("code-block", "```rust\nfn main() {}\n```"),
        ("ordered-list", "1. first"),
        ("horizontal-line", "---"),
        ("table", "| a |\n| - |\n| 1 |"),
        ("query", "open tasks"),
    ] {
        leaves.push((leaf_type, create(&svc, leaf_type, content).await));
    }
    for (leaf_type, leaf) in leaves {
        assert_eq!(
            refused(create_under(&svc, &leaf, "text", "child").await),
            TreeInvariantRule::ChildrenNone,
            "create under a {leaf_type}"
        );
        assert_eq!(
            refused(move_under(&svc, &note, Some(&leaf)).await),
            TreeInvariantRule::ChildrenNone,
            "move under a {leaf_type}"
        );
        assert_eq!(
            refused(
                svc.create_relationship(&leaf, "has_child", &note, json!({ "order": 1.0 }))
                    .await
            ),
            TreeInvariantRule::ChildrenNone,
            "relationship under a {leaf_type}"
        );
        assert!(svc.get_children(&leaf).await.unwrap().is_empty());
        // A leaf may itself be a child.
        move_under(&svc, &leaf, Some(&note)).await.unwrap();
    }
}

/// A schema's structural rules change only through `update_schema`: the
/// database enforces whatever a schema node declares, so a generic update
/// must not install a rule that skipped the checks.
#[tokio::test]
async fn a_generic_update_cannot_change_a_schemas_rules() {
    let (svc, _tmp) = test_service().await;
    create_schema(&svc, json!({ "name": "Folder", "fields": [] })).await;
    let page = create(&svc, "text", "A page").await;
    create_under(&svc, &page, "folder", "nested").await.unwrap();

    let version = svc.get_node("folder").await.unwrap().unwrap().version;
    for properties in [
        json!({ "parent": { "rule": "must_be_root" } }),
        json!({ "children": { "rule": "none" } }),
    ] {
        let error = svc
            .update_node(
                "folder",
                version,
                NodeUpdate::new().with_properties(properties.clone()),
            )
            .await
            .expect_err("a generic update must not change a schema's rules")
            .to_string();
        assert!(error.contains("update_schema"), "{properties}: {error}");
    }
    assert_eq!(
        svc.store()
            .structural_rules_in_force("folder")
            .await
            .unwrap(),
        (SchemaChildrenRule::Any, SchemaParentRule::Any)
    );

    // Any other property of the schema node is still a generic update away.
    svc.update_node(
        "folder",
        version,
        NodeUpdate::new().with_content("Folders".to_string()),
    )
    .await
    .unwrap();
}

/// A rule the database cannot read is refused when the schema is written,
/// rather than stored and read as `any`.
#[tokio::test]
async fn a_malformed_rule_is_refused() {
    let (svc, _tmp) = test_service().await;
    for properties in [
        json!({ "fields": [], "children": { "rule": "sometimes" } }),
        json!({ "fields": [], "children": "none" }),
        json!({ "fields": [], "parent": { "rule": "must_have_parent_of" } }),
        json!({ "fields": [], "parent": { "rule": "must_have_parent_of", "types": [] } }),
    ] {
        let result = svc
            .create_node(Node::new_with_id(
                "widget".to_string(),
                "schema".to_string(),
                "Widget".to_string(),
                properties.clone(),
            ))
            .await;
        assert!(result.is_err(), "{properties} must be refused");
        assert!(svc.store().get_node("widget").await.unwrap().is_none());
    }
}

// ---------------------------------------------------------------------------
// Every write path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_bulk_import_is_held_to_both_rules() {
    let (svc, _tmp) = test_service().await;
    seed_user_rules(&svc).await;
    let id = || uuid::Uuid::new_v4().to_string();
    let row = |id: &str, node_type: &str, parent: Option<&str>| {
        (
            id.to_string(),
            node_type.to_string(),
            "content".to_string(),
            parent.map(str::to_string),
            1.0,
            json!({}),
        )
    };

    // A child under a leaf, both in the batch.
    let (root, leaf, child) = (id(), id(), id());
    let result = svc
        .bulk_create_hierarchy(vec![
            row(&root, "text", None),
            row(&leaf, "query", Some(&root)),
            row(&child, "text", Some(&leaf)),
        ])
        .await;
    assert_eq!(refused(result), TreeInvariantRule::ChildrenNone);
    assert!(
        svc.get_node(&root).await.unwrap().is_none(),
        "a refused import writes nothing"
    );

    // A root-only type under a parent already stored.
    let page = create(&svc, "text", "A page").await;
    let result = svc
        .bulk_create_hierarchy(vec![row(&id(), "collection", Some(&page))])
        .await;
    assert_eq!(refused(result), TreeInvariantRule::MustBeRoot);

    // A type that needs a parent, imported as a root or under the wrong one.
    for parent in [None, Some(page.as_str())] {
        let result = svc
            .bulk_create_hierarchy(vec![row(&id(), "reply", parent)])
            .await;
        assert_eq!(refused(result), TreeInvariantRule::ParentRequired);
    }

    // A tree the rules allow goes in whole.
    let (thread, reply) = (id(), id());
    svc.bulk_create_hierarchy(vec![
        row(&thread, "thread", None),
        row(&reply, "reply", Some(&thread)),
    ])
    .await
    .unwrap();
    assert_eq!(parent_of(&svc, &reply).await, Some(thread));
}

#[tokio::test]
async fn a_generic_has_child_relationship_is_held_to_both_rules() {
    let (svc, _tmp) = test_service().await;
    seed_user_rules(&svc).await;
    let page = create(&svc, "text", "A page").await;
    let journal = create(&svc, "journal", "Mine").await;
    let task = create(&svc, "task", "Do it").await;
    let collection = create(&svc, "collection", "Work").await;

    // With and without an explicit order: the two store paths.
    for edge_data in [json!({}), json!({ "order": 1.0 })] {
        assert_eq!(
            refused(
                svc.create_relationship(&page, "has_child", &collection, edge_data.clone())
                    .await
            ),
            TreeInvariantRule::MustBeRoot
        );
        assert_eq!(
            refused(
                svc.create_relationship(&journal, "has_child", &task, edge_data)
                    .await
            ),
            TreeInvariantRule::ChildNotAllowed
        );
    }
    assert_eq!(parent_of(&svc, &task).await, None);
}

#[tokio::test]
async fn a_retype_is_checked_against_the_nodes_parent_and_children() {
    let (svc, _tmp) = test_service().await;
    seed_user_rules(&svc).await;
    let page = create(&svc, "text", "A page").await;
    let line = create_under(&svc, &page, "text", "A line").await.unwrap();
    create_under(&svc, &line, "text", "Under the line")
        .await
        .unwrap();

    // The new type must be a root, but the node has a parent.
    assert_eq!(
        refused(retype(&svc, &line, "collection").await),
        TreeInvariantRule::MustBeRoot
    );
    // The new type takes no children, but the node has one.
    assert_eq!(
        refused(retype(&svc, &line, "query").await),
        TreeInvariantRule::ChildrenNone
    );
    // The new type needs a thread above it.
    assert_eq!(
        refused(retype(&svc, &line, "reply").await),
        TreeInvariantRule::ParentRequired
    );
    assert_eq!(
        refused(retype(&svc, &page, "reply").await),
        TreeInvariantRule::ParentRequired,
        "a root cannot take a type that needs a parent"
    );

    // The parent's rule is checked against the child's new type.
    let journal = create(&svc, "journal", "Mine").await;
    let entry = create_under(&svc, &journal, "text", "An entry")
        .await
        .unwrap();
    assert_eq!(
        refused(retype(&svc, &entry, "task").await),
        TreeInvariantRule::ChildNotAllowed
    );
    // And a child's rule against its parent's new type.
    let thread = create(&svc, "thread", "General").await;
    create_under(&svc, &thread, "reply", "hello").await.unwrap();
    assert_eq!(
        refused(retype(&svc, &thread, "text").await),
        TreeInvariantRule::ParentRequired
    );

    assert_eq!(
        svc.get_node(&line).await.unwrap().unwrap().node_type,
        "text"
    );
    // A retype the rules allow goes through.
    retype(&svc, &line, "header").await.unwrap();
}

// ---------------------------------------------------------------------------
// A type that needs a parent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_type_that_needs_a_parent_is_never_created_or_left_without_one() {
    let (svc, _tmp) = test_service().await;
    seed_user_rules(&svc).await;
    let page = create(&svc, "text", "A page").await;
    let thread = create(&svc, "thread", "General").await;
    let roots_before = svc.count_roots(false).await.unwrap();

    // Created as a root, on the single and the bulk path: refused before
    // anything is written.
    assert_eq!(
        refused(try_create(&svc, "reply", "orphan").await),
        TreeInvariantRule::ParentRequired
    );
    assert_eq!(
        refused(
            svc.bulk_create(vec![Node::new(
                "reply".to_string(),
                "orphan".to_string(),
                json!({})
            )])
            .await
        ),
        TreeInvariantRule::ParentRequired
    );
    // Created under a parent that does not qualify.
    assert_eq!(
        refused(create_under(&svc, &page, "reply", "misplaced").await),
        TreeInvariantRule::ParentRequired
    );
    assert_eq!(svc.count_roots(false).await.unwrap(), roots_before);
    assert!(svc.get_children(&page).await.unwrap().is_empty());

    // Under a thread it is created, and then cannot leave for anywhere else.
    let reply = create_under(&svc, &thread, "reply", "hello").await.unwrap();
    assert_eq!(
        refused(move_under(&svc, &reply, None).await),
        TreeInvariantRule::ParentRequired
    );
    assert_eq!(
        refused(move_under(&svc, &reply, Some(&page)).await),
        TreeInvariantRule::ParentRequired
    );
    assert_eq!(
        refused(svc.delete_relationship(&thread, "has_child", &reply).await),
        TreeInvariantRule::ParentRequired
    );
    assert_eq!(parent_of(&svc, &reply).await, Some(thread.clone()));
    // It takes no children of its own.
    assert_eq!(
        refused(create_under(&svc, &reply, "text", "nested").await),
        TreeInvariantRule::ChildrenNone
    );

    // Another thread qualifies.
    let other = create(&svc, "thread", "Other").await;
    move_under(&svc, &reply, Some(&other)).await.unwrap();
}

// ---------------------------------------------------------------------------
// Chats
// ---------------------------------------------------------------------------

/// A chat may have children, and a child of a chat is its own embedding root:
/// the chat's subtree is not embedded, so it would otherwise be unsearchable.
#[tokio::test]
async fn a_child_of_a_chat_is_its_own_embedding_root() {
    let (svc, _tmp) = test_service().await;
    // Both chat subtypes inherit the base's rule; a chat names who runs it.
    let new_chat = |node_type: &str, content: &str| {
        Node::new(
            node_type.to_string(),
            content.to_string(),
            json!({ "agent": "nodespace" }),
        )
    };
    let chat = svc
        .create_node(new_chat("ai-chat-native", "Planning chat"))
        .await
        .unwrap();
    let note = create_under(&svc, &chat, "text", "A note kept in the chat")
        .await
        .expect("a chat may have children");
    let detail = create_under(&svc, &note, "text", "A detail under the note")
        .await
        .unwrap();

    assert_eq!(svc.get_embedding_root_id(&chat).await.unwrap(), chat);
    assert_eq!(svc.get_embedding_root_id(&note).await.unwrap(), note);
    assert_eq!(svc.get_embedding_root_id(&detail).await.unwrap(), note);

    // The same holds for a chat that is itself nested.
    let page = create(&svc, "text", "A page").await;
    let nested_chat = svc
        .create_node(new_chat("ai-chat-pty", "Nested chat"))
        .await
        .unwrap();
    move_under(&svc, &nested_chat, Some(&page)).await.unwrap();
    let nested_note = create_under(&svc, &nested_chat, "text", "Kept in the nested chat")
        .await
        .unwrap();
    assert_eq!(svc.get_embedding_root_id(&nested_chat).await.unwrap(), page);
    assert_eq!(
        svc.get_embedding_root_id(&nested_note).await.unwrap(),
        nested_note
    );
}
