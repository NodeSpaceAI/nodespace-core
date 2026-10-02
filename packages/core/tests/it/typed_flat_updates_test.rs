//! The typed updates of the flat core types, against a real store (ADR-086
//! §4): each writes and clears its fields through the shared update pipeline,
//! conflicts on a stale version, and refuses a node of another type.

use nodespace_core::db::SqliteStore;
use nodespace_core::models::{
    node_to_typed_value, CollectionNodeUpdate, DatabaseSettingsNodeUpdate, Node, Priority,
    ProjectNodeUpdate, ProjectStatus, QueryNodeUpdate, SkillFields, SkillNodeUpdate,
};
use nodespace_core::services::{NodeService, NodeServiceError};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

const SETTINGS_ID: &str = "database-settings-singleton";

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

async fn create(
    svc: &NodeService,
    node_type: &str,
    content: &str,
    properties: serde_json::Value,
) -> Node {
    let id = svc
        .create_node(Node::new(
            node_type.to_string(),
            content.to_string(),
            properties,
        ))
        .await
        .unwrap_or_else(|e| panic!("creating a {node_type} failed: {e:?}"));
    stored(svc, &id).await
}

async fn stored(svc: &NodeService, id: &str) -> Node {
    svc.get_node(id).await.unwrap().expect("node exists")
}

async fn typed(svc: &NodeService, id: &str) -> serde_json::Value {
    node_to_typed_value(stored(svc, id).await).unwrap()
}

fn assert_conflict(result: Result<Node, NodeServiceError>, id: &str, expected: i64, actual: i64) {
    match result {
        Err(NodeServiceError::VersionConflict {
            node_id,
            expected_version,
            actual_version,
        }) => {
            assert_eq!(node_id, id);
            assert_eq!(expected_version, expected);
            assert_eq!(actual_version, actual);
        }
        other => panic!("expected a version conflict, got {other:?}"),
    }
}

fn assert_wrong_type(result: Result<Node, NodeServiceError>, expected: &str) {
    let error = result
        .expect_err("a node of another type must be refused")
        .to_string();
    assert!(
        error.contains(&format!("not a {expected} node")),
        "expected a wrong-type refusal for {expected}, got: {error}"
    );
}

// ---------------------------------------------------------------------------
// collection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_collection_update_writes_and_clears_its_description() {
    let (svc, _tmp) = test_service().await;
    let collection = create(&svc, "collection", "clients", json!({})).await;

    let updated = svc
        .update_collection_node(
            &collection.id,
            collection.version,
            CollectionNodeUpdate {
                description: Some(Some("Accounts we bill".to_string())),
            },
        )
        .await
        .expect("setting the description succeeds");
    assert_eq!(updated.version, collection.version + 1);
    assert_eq!(
        updated.properties["collection"]["description"],
        "Accounts we bill"
    );
    // The name is untouched, and so is the title computed from it.
    assert_eq!(updated.content, "clients");
    assert_eq!(updated.title.as_deref(), Some("clients"));
    assert_eq!(
        typed(&svc, &collection.id).await["description"],
        "Accounts we bill"
    );

    let cleared = svc
        .update_collection_node(
            &collection.id,
            updated.version,
            CollectionNodeUpdate {
                description: Some(None),
            },
        )
        .await
        .expect("clearing the description succeeds");
    assert!(cleared.properties["collection"]["description"].is_null());
    assert!(typed(&svc, &collection.id)
        .await
        .get("description")
        .is_none());
}

#[tokio::test]
async fn a_collection_update_conflicts_on_a_stale_version_and_refuses_another_type() {
    let (svc, _tmp) = test_service().await;
    let collection = create(&svc, "collection", "clients", json!({})).await;
    let update = || CollectionNodeUpdate {
        description: Some(Some("Accounts we bill".to_string())),
    };

    svc.update_collection_node(&collection.id, collection.version, update())
        .await
        .unwrap();
    assert_conflict(
        svc.update_collection_node(&collection.id, collection.version, update())
            .await,
        &collection.id,
        collection.version,
        collection.version + 1,
    );

    let text = create(&svc, "text", "A line", json!({})).await;
    assert_wrong_type(
        svc.update_collection_node(&text.id, text.version, update())
            .await,
        "collection",
    );

    let error = svc
        .update_collection_node(
            &collection.id,
            collection.version + 1,
            CollectionNodeUpdate::default(),
        )
        .await
        .expect_err("an empty update is refused")
        .to_string();
    assert!(error.contains("contains no changes"), "{error}");
}

// ---------------------------------------------------------------------------
// skill
// ---------------------------------------------------------------------------

async fn create_skill(svc: &NodeService) -> Node {
    let node = SkillFields::new("Update a record", &["update_node", "get_node"], 3)
        .with_exclusion("Delete records")
        .with_node_types(&["task"])
        .into_node("Graph Editing");
    let id = svc
        .create_node(node)
        .await
        .expect("creating a skill succeeds");
    stored(svc, &id).await
}

#[tokio::test]
async fn a_skill_update_writes_and_clears_its_fields() {
    let (svc, _tmp) = test_service().await;
    let skill = create_skill(&svc).await;

    let updated = svc
        .update_skill_node(
            &skill.id,
            skill.version,
            SkillNodeUpdate {
                description: Some("Change a record".to_string()),
                tool_whitelist: Some(vec!["update_node".to_string()]),
                max_iterations: Some(Some(5)),
                ..Default::default()
            },
        )
        .await
        .expect("setting skill fields succeeds");
    assert_eq!(updated.version, skill.version + 1);
    assert_eq!(
        SkillFields::from_node(&updated).unwrap(),
        SkillFields::new("Change a record", &["update_node"], 5)
            .with_exclusion("Delete records")
            .with_node_types(&["task"]),
        "the fields the update did not name are kept"
    );
    assert_eq!(updated.content, "Graph Editing");

    let cleared = svc
        .update_skill_node(
            &skill.id,
            updated.version,
            SkillNodeUpdate {
                exclusion: Some(None),
                max_iterations: Some(None),
                node_types: Some(None),
                ..Default::default()
            },
        )
        .await
        .expect("clearing skill fields succeeds");
    assert!(cleared.properties["skill"]["exclusion"].is_null());
    // A cleared field reads as the schema's default.
    assert_eq!(
        SkillFields::from_node(&cleared).unwrap(),
        SkillFields::new("Change a record", &["update_node"], 2)
    );

    let wire = typed(&svc, &skill.id).await;
    assert_eq!(wire["description"], "Change a record");
    assert!(wire.get("exclusion").is_none());
    assert_eq!(wire["toolWhitelist"], json!(["update_node"]));
    assert_eq!(wire["maxIterations"], 2);
    assert_eq!(wire["nodeTypes"], json!([]));
    assert_eq!(wire["properties"], json!({}));
}

#[tokio::test]
async fn a_skill_update_conflicts_on_a_stale_version_and_refuses_another_type() {
    let (svc, _tmp) = test_service().await;
    let skill = create_skill(&svc).await;
    let update = || SkillNodeUpdate::description("Change a record");

    svc.update_skill_node(&skill.id, skill.version, update())
        .await
        .unwrap();
    assert_conflict(
        svc.update_skill_node(&skill.id, skill.version, update())
            .await,
        &skill.id,
        skill.version,
        skill.version + 1,
    );

    let text = create(&svc, "text", "A line", json!({})).await;
    assert_wrong_type(
        svc.update_skill_node(&text.id, text.version, update())
            .await,
        "skill",
    );
}

/// The typed update lowers into the shared pipeline, so the skill behaviour
/// checks the resulting fields as it does for any other write.
#[tokio::test]
async fn a_skill_update_is_validated_by_the_shared_pipeline() {
    let (svc, _tmp) = test_service().await;
    let skill = create_skill(&svc).await;

    let error = svc
        .update_skill_node(
            &skill.id,
            skill.version,
            SkillNodeUpdate {
                max_iterations: Some(Some(0)),
                ..Default::default()
            },
        )
        .await
        .expect_err("a zero iteration budget is refused")
        .to_string();
    assert!(error.contains("max_iterations"), "{error}");
    assert_eq!(stored(&svc, &skill.id).await.version, skill.version);
}

// ---------------------------------------------------------------------------
// database-settings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_database_settings_update_writes_and_clears_required_extensions() {
    let (svc, _tmp) = test_service().await;
    let settings = stored(&svc, SETTINGS_ID).await;
    assert_eq!(
        typed(&svc, SETTINGS_ID).await["requiredExtensions"],
        json!([])
    );

    let updated = svc
        .update_database_settings_node(
            SETTINGS_ID,
            settings.version,
            DatabaseSettingsNodeUpdate {
                required_extensions: Some(Some(vec!["fixture".to_string()])),
            },
        )
        .await
        .expect("setting the list succeeds");
    assert_eq!(updated.version, settings.version + 1);
    assert_eq!(
        updated.properties["database-settings"]["required_extensions"],
        json!(["fixture"])
    );
    let wire = typed(&svc, SETTINGS_ID).await;
    assert_eq!(wire["requiredExtensions"], json!(["fixture"]));
    assert_eq!(wire["properties"], json!({}));

    let cleared = svc
        .update_database_settings_node(
            SETTINGS_ID,
            updated.version,
            DatabaseSettingsNodeUpdate {
                required_extensions: Some(None),
            },
        )
        .await
        .expect("clearing the list succeeds");
    assert!(cleared.properties["database-settings"]["required_extensions"].is_null());
    assert_eq!(
        typed(&svc, SETTINGS_ID).await["requiredExtensions"],
        json!([])
    );
}

#[tokio::test]
async fn a_database_settings_update_conflicts_on_a_stale_version_and_refuses_another_type() {
    let (svc, _tmp) = test_service().await;
    let settings = stored(&svc, SETTINGS_ID).await;
    let update = || DatabaseSettingsNodeUpdate {
        required_extensions: Some(Some(vec!["fixture".to_string()])),
    };

    svc.update_database_settings_node(SETTINGS_ID, settings.version, update())
        .await
        .unwrap();
    assert_conflict(
        svc.update_database_settings_node(SETTINGS_ID, settings.version, update())
            .await,
        SETTINGS_ID,
        settings.version,
        settings.version + 1,
    );

    let text = create(&svc, "text", "A line", json!({})).await;
    assert_wrong_type(
        svc.update_database_settings_node(&text.id, text.version, update())
            .await,
        "database-settings",
    );
}

// ---------------------------------------------------------------------------
// project
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_project_update_conflicts_on_a_stale_version() {
    let (svc, _tmp) = test_service().await;
    let project = create(&svc, "project", "Apollo", json!({})).await;
    let update = || ProjectNodeUpdate {
        status: Some(ProjectStatus::Active),
        ..Default::default()
    };

    let updated = svc
        .update_project_node(&project.id, project.version, update())
        .await
        .expect("a current version writes");
    assert_eq!(updated.properties["project"]["status"], "active");

    assert_conflict(
        svc.update_project_node(
            &project.id,
            project.version,
            ProjectNodeUpdate {
                status: Some(ProjectStatus::Completed),
                ..Default::default()
            },
        )
        .await,
        &project.id,
        project.version,
        project.version + 1,
    );
    // The stale write changed nothing.
    assert_eq!(typed(&svc, &project.id).await["status"], "active");
}

/// `status` and `priority` are extensible enums: a value outside the core
/// ones is a user value, accepted only once the schema declares it.
#[tokio::test]
async fn a_project_update_refuses_a_value_the_schema_does_not_declare() {
    let (svc, _tmp) = test_service().await;
    let project = create(&svc, "project", "Apollo", json!({})).await;

    for (update, field) in [
        (
            ProjectNodeUpdate {
                status: Some(ProjectStatus::User("on_hold".to_string())),
                ..Default::default()
            },
            "status",
        ),
        (
            ProjectNodeUpdate {
                priority: Some(Some(Priority::User("urgent".to_string()))),
                ..Default::default()
            },
            "priority",
        ),
    ] {
        let error = svc
            .update_project_node(&project.id, project.version, update)
            .await
            .expect_err("an undeclared value is refused")
            .to_string();
        assert!(error.contains(field), "{field}: {error}");
    }
    assert_eq!(stored(&svc, &project.id).await.version, project.version);

    // A core value of the shared priority scale is accepted.
    let updated = svc
        .update_project_node(
            &project.id,
            project.version,
            ProjectNodeUpdate {
                priority: Some(Some(Priority::Highest)),
                ..Default::default()
            },
        )
        .await
        .expect("a core priority writes");
    assert_eq!(updated.properties["project"]["priority"], "highest");
}

// ---------------------------------------------------------------------------
// query
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_query_update_conflicts_on_a_stale_version() {
    let (svc, _tmp) = test_service().await;
    let query = create(
        &svc,
        "query",
        "Open tasks",
        json!({ "target_type": "task" }),
    )
    .await;
    let update = |limit: usize| QueryNodeUpdate {
        limit: Some(Some(limit)),
        ..Default::default()
    };

    svc.update_query_node(&query.id, query.version, update(10))
        .await
        .expect("a current version writes");

    assert_conflict(
        svc.update_query_node(&query.id, query.version, update(20))
            .await,
        &query.id,
        query.version,
        query.version + 1,
    );
    // The stale write changed nothing.
    assert_eq!(typed(&svc, &query.id).await["limit"], 10);
}
