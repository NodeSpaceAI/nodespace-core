//! The typed updates of the flat core types, against a real store (ADR-086
//! §4): each writes and clears its fields through the shared update pipeline,
//! conflicts on a stale version, and refuses a node of another type.

use nodespace_core::db::SqliteStore;
use nodespace_core::models::{
    node_to_typed_value, CollectionNodeUpdate, DatabaseSettingsNodeUpdate, DecisionNodeUpdate,
    DecisionStatus, LinkValue, Node, NodeUpdate, PlanNodeUpdate, PlanStatus, Priority,
    ProjectNodeUpdate, ProjectStatus, QueryNodeUpdate, SkillFields, SkillNodeUpdate,
    SpecNodeUpdate, SpecStatus, TaskNodeUpdate, TaskStatus,
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
        SkillFields::new("Change a record", &["update_node"], 5).with_exclusion("Delete records"),
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
    assert!(wire.get("nodeTypes").is_none());
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

/// The schemas a skill is about are its `applies_to` edges. A list of schema
/// ids on the skill is an undeclared key, which the closed schema refuses.
#[tokio::test]
async fn a_skill_refuses_a_list_of_schema_ids_as_an_undeclared_key() {
    let (svc, _tmp) = test_service().await;
    let mut node =
        SkillFields::new("Update a record", &["update_node"], 3).into_node("Graph Editing");
    node.properties["node_types"] = json!(["task"]);

    let error = svc
        .create_node(node)
        .await
        .expect_err("an undeclared skill key is refused")
        .to_string();
    assert!(error.contains("node_types"), "{error}");
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

fn link(title: &str, url: &str) -> LinkValue {
    LinkValue {
        title: title.to_string(),
        url: url.to_string(),
    }
}

/// `repository` is a link: set, read back typed, and cleared.
#[tokio::test]
async fn a_project_update_writes_and_clears_its_repository() {
    let (svc, _tmp) = test_service().await;
    let project = create(&svc, "project", "Apollo", json!({})).await;

    let updated = svc
        .update_project_node(
            &project.id,
            project.version,
            ProjectNodeUpdate {
                repository: Some(Some(link("core", "https://github.com/acme/core"))),
                ..Default::default()
            },
        )
        .await
        .expect("setting the repository succeeds");
    assert_eq!(
        updated.properties["project"]["repository"],
        json!({ "title": "core", "url": "https://github.com/acme/core" })
    );
    let wire = typed(&svc, &project.id).await;
    assert_eq!(
        wire["repository"],
        json!({ "title": "core", "url": "https://github.com/acme/core" })
    );
    assert!(wire["properties"].get("repository").is_none());

    // A value that is not an absolute URL is refused by the shared pipeline.
    let error = svc
        .update_project_node(
            &project.id,
            updated.version,
            ProjectNodeUpdate {
                repository: Some(Some(link("core", "acme/core"))),
                ..Default::default()
            },
        )
        .await
        .expect_err("a relative URL is not a link")
        .to_string();
    assert!(error.contains("repository"), "{error}");

    let cleared = svc
        .update_project_node(
            &project.id,
            updated.version,
            ProjectNodeUpdate {
                repository: Some(None),
                ..Default::default()
            },
        )
        .await
        .expect("clearing the repository succeeds");
    assert!(cleared.properties["project"]["repository"].is_null());
    assert!(typed(&svc, &project.id).await.get("repository").is_none());
}

// ---------------------------------------------------------------------------
// task
// ---------------------------------------------------------------------------

/// A task records where its work landed: one pull request and a list of
/// commits, written whole.
#[tokio::test]
async fn a_task_update_writes_and_clears_its_links() {
    let (svc, _tmp) = test_service().await;
    let task = create(&svc, "task", "Ship it", json!({ "status": "open" })).await;

    let updated = svc
        .update_task_node(
            &task.id,
            task.version,
            TaskNodeUpdate {
                status: Some(TaskStatus::InReview),
                pull_request: Some(Some(link("PR 7", "https://example.com/pull/7"))),
                commits: Some(Some(vec![
                    link("abc123", "https://example.com/commit/abc123"),
                    link("def456", "https://example.com/commit/def456"),
                ])),
                ..Default::default()
            },
        )
        .await
        .expect("setting the links succeeds");
    let wire = typed(&svc, &task.id).await;
    assert_eq!(wire["status"], "in_review");
    assert_eq!(
        wire["pullRequest"],
        json!({ "title": "PR 7", "url": "https://example.com/pull/7" })
    );
    assert_eq!(wire["commits"].as_array().map(Vec::len), Some(2));
    assert_eq!(wire["commits"][1]["title"], "def456");
    assert_eq!(wire["properties"], json!({}));

    // The list is replaced whole, never appended to.
    let replaced = svc
        .update_task_node(
            &task.id,
            updated.version,
            TaskNodeUpdate {
                commits: Some(Some(vec![link(
                    "fff000",
                    "https://example.com/commit/fff000",
                )])),
                ..Default::default()
            },
        )
        .await
        .expect("replacing the commits succeeds");
    assert_eq!(
        replaced.properties["task"]["commits"],
        json!([{ "title": "fff000", "url": "https://example.com/commit/fff000" }])
    );

    // An item that is not a link is refused, and nothing is written.
    let error = svc
        .update_node(
            &task.id,
            replaced.version,
            NodeUpdate::default().with_properties(json!({ "commits": ["fff000"] })),
        )
        .await
        .expect_err("a bare string is not a link")
        .to_string();
    assert!(error.contains("commits"), "{error}");

    let cleared = svc
        .update_task_node(
            &task.id,
            replaced.version,
            TaskNodeUpdate {
                pull_request: Some(None),
                commits: Some(None),
                ..Default::default()
            },
        )
        .await
        .expect("clearing the links succeeds");
    assert!(cleared.properties["task"]["pull_request"].is_null());
    let wire = typed(&svc, &task.id).await;
    assert!(wire.get("pullRequest").is_none());
    assert!(wire.get("commits").is_none());
}

/// `in_review` is a core status, and a status outside the vocabulary is
/// still refused.
#[tokio::test]
async fn a_task_takes_in_review_and_refuses_an_undeclared_status() {
    let (svc, _tmp) = test_service().await;
    let task = create(&svc, "task", "Ship it", json!({})).await;
    assert_eq!(typed(&svc, &task.id).await["status"], "open");

    let updated = svc
        .update_task_node(
            &task.id,
            task.version,
            TaskNodeUpdate {
                status: Some(TaskStatus::InReview),
                ..Default::default()
            },
        )
        .await
        .expect("in_review is a core status");
    assert_eq!(updated.properties["task"]["status"], "in_review");

    let error = svc
        .update_task_node(
            &task.id,
            updated.version,
            TaskNodeUpdate {
                status: Some(TaskStatus::User("awaiting_qa".to_string())),
                ..Default::default()
            },
        )
        .await
        .expect_err("an undeclared status is refused")
        .to_string();
    assert!(error.contains("status"), "{error}");
}

// ---------------------------------------------------------------------------
// spec, plan, decision
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_spec_update_writes_and_clears_its_fields() {
    let (svc, _tmp) = test_service().await;
    let spec = create(&svc, "spec", "Offline mode", json!({})).await;
    // A new spec is a draft, and its wire shape is typed with no fields left
    // in `properties`.
    let wire = typed(&svc, &spec.id).await;
    assert_eq!(wire["nodeType"], "spec");
    assert_eq!(wire["specStatus"], "draft");
    assert_eq!(wire["properties"], json!({}));
    assert!(wire.get("objective").is_none());

    let updated = svc
        .update_spec_node(
            &spec.id,
            spec.version,
            SpecNodeUpdate {
                objective: Some(Some("Work without a network".to_string())),
                boundaries: Some(Some("Never drop a write".to_string())),
                spec_status: None,
            },
        )
        .await
        .expect("writing a spec's fields succeeds");
    assert_eq!(updated.version, spec.version + 1);
    assert_eq!(updated.content, "Offline mode");
    assert_eq!(updated.title.as_deref(), Some("Offline mode"));
    let wire = typed(&svc, &spec.id).await;
    assert_eq!(wire["objective"], "Work without a network");
    assert_eq!(wire["boundaries"], "Never drop a write");
    assert_eq!(wire["specStatus"], "draft");
    assert_eq!(wire["properties"], json!({}));

    // The status is written on its own, as a client writes it: with the
    // rules running, one write that edits a field and supersedes is refused
    // (`editing_and_superseding_in_one_write_is_refused` in the Play tests).
    // No engine runs here.
    svc.update_spec_node(
        &spec.id,
        updated.version,
        SpecNodeUpdate {
            spec_status: Some(SpecStatus::Superseded),
            ..Default::default()
        },
    )
    .await
    .expect("writing a spec's status succeeds");
    assert_eq!(typed(&svc, &spec.id).await["specStatus"], "superseded");

    let fresh = create(&svc, "spec", "Sync", json!({ "objective": "Share" })).await;
    let cleared = svc
        .update_spec_node(
            &fresh.id,
            fresh.version,
            SpecNodeUpdate {
                objective: Some(None),
                ..Default::default()
            },
        )
        .await
        .expect("clearing the objective succeeds");
    assert!(cleared.properties["spec"]["objective"].is_null());
    assert!(typed(&svc, &fresh.id).await.get("objective").is_none());
}

#[tokio::test]
async fn a_spec_update_conflicts_on_a_stale_version_and_refuses_another_type() {
    let (svc, _tmp) = test_service().await;
    let spec = create(&svc, "spec", "Offline mode", json!({})).await;
    let update = || SpecNodeUpdate {
        objective: Some(Some("Work without a network".to_string())),
        ..Default::default()
    };

    svc.update_spec_node(&spec.id, spec.version, update())
        .await
        .unwrap();
    assert_conflict(
        svc.update_spec_node(&spec.id, spec.version, update()).await,
        &spec.id,
        spec.version,
        spec.version + 1,
    );

    let plan = create(&svc, "plan", "Queue writes", json!({})).await;
    assert_wrong_type(
        svc.update_spec_node(&plan.id, plan.version, update()).await,
        "spec",
    );
    assert!(svc
        .update_spec_node(&spec.id, spec.version + 1, SpecNodeUpdate::default())
        .await
        .is_err());
}

#[tokio::test]
async fn a_plan_update_writes_clears_conflicts_and_refuses_another_type() {
    let (svc, _tmp) = test_service().await;
    let plan = create(&svc, "plan", "Queue writes", json!({ "risks": "Ordering" })).await;
    assert_eq!(typed(&svc, &plan.id).await["planStatus"], "draft");

    let updated = svc
        .update_plan_node(
            &plan.id,
            plan.version,
            PlanNodeUpdate {
                approach: Some(Some("A local log, replayed on reconnect".to_string())),
                risks: Some(None),
                plan_status: None,
            },
        )
        .await
        .expect("writing a plan's fields succeeds");
    assert!(updated.properties["plan"]["risks"].is_null());
    let wire = typed(&svc, &plan.id).await;
    assert_eq!(wire["approach"], "A local log, replayed on reconnect");
    assert_eq!(wire["planStatus"], "draft");
    assert!(wire.get("risks").is_none());
    assert_eq!(wire["properties"], json!({}));

    // The status is written on its own, as for a spec.
    let other = create(&svc, "plan", "Batch writes", json!({})).await;
    svc.update_plan_node(
        &other.id,
        other.version,
        PlanNodeUpdate {
            plan_status: Some(PlanStatus::Superseded),
            ..Default::default()
        },
    )
    .await
    .expect("writing a plan's status succeeds");
    assert_eq!(typed(&svc, &other.id).await["planStatus"], "superseded");

    let stale = || PlanNodeUpdate {
        approach: Some(Some("Something else".to_string())),
        ..Default::default()
    };
    assert_conflict(
        svc.update_plan_node(&plan.id, plan.version, stale()).await,
        &plan.id,
        plan.version,
        plan.version + 1,
    );
    let spec = create(&svc, "spec", "Offline mode", json!({})).await;
    assert_wrong_type(
        svc.update_plan_node(&spec.id, spec.version, stale()).await,
        "plan",
    );
}

#[tokio::test]
async fn a_decision_update_writes_conflicts_and_refuses_another_type() {
    let (svc, _tmp) = test_service().await;
    let decision = create(&svc, "decision", "Use SQLite", json!({})).await;
    let wire = typed(&svc, &decision.id).await;
    assert_eq!(wire["decisionStatus"], "proposed");
    assert_eq!(wire["properties"], json!({}));

    let accept = || DecisionNodeUpdate {
        decision_status: Some(DecisionStatus::Accepted),
    };
    let updated = svc
        .update_decision_node(&decision.id, decision.version, accept())
        .await
        .expect("accepting a decision succeeds");
    assert_eq!(
        updated.properties["decision"]["decision_status"],
        "accepted"
    );
    assert_eq!(
        typed(&svc, &decision.id).await["decisionStatus"],
        "accepted"
    );

    assert_conflict(
        svc.update_decision_node(&decision.id, decision.version, accept())
            .await,
        &decision.id,
        decision.version,
        decision.version + 1,
    );
    let spec = create(&svc, "spec", "Offline mode", json!({})).await;
    assert_wrong_type(
        svc.update_decision_node(&spec.id, spec.version, accept())
            .await,
        "decision",
    );
}

/// The three schemas are closed, and their status vocabularies are too: an
/// undeclared key, a retired field and a value outside the vocabulary are
/// each refused on the generic write path the CLI and the agent use.
#[tokio::test]
async fn the_model_types_refuse_undeclared_keys_and_values() {
    let (svc, _tmp) = test_service().await;
    for (node_type, patch) in [
        ("spec", json!({ "success_criteria": "It works" })),
        ("spec", json!({ "spec_status": "in_review" })),
        ("plan", json!({ "owner": "ada" })),
        ("plan", json!({ "plan_status": "accepted" })),
        ("decision", json!({ "rationale": "Because" })),
        ("decision", json!({ "decision_status": "approved" })),
        ("task", json!({ "verification_method": "Ran the tests" })),
    ] {
        let node = create(&svc, node_type, "A node", json!({})).await;
        let result = svc
            .update_node(
                &node.id,
                node.version,
                NodeUpdate::default().with_properties(patch.clone()),
            )
            .await;
        assert!(result.is_err(), "{node_type}: {patch} must be refused");
        assert_eq!(stored(&svc, &node.id).await.version, node.version);
    }

    // A title is required: each of the three is named by its content.
    for node_type in ["spec", "plan", "decision"] {
        let result = svc
            .create_node(Node::new(
                node_type.to_string(),
                "  ".to_string(),
                json!({}),
            ))
            .await;
        assert!(result.is_err(), "an untitled {node_type} must be refused");
    }
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
